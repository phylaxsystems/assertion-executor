use crate::{
    db::DatabaseRef,
    primitives::{
        Address,
        AssertionContract,
        Bytecode,
        Bytes,
        FixedBytes,
        SpecId,
        B256,
    },
    store::{
        AssertionContractExtractor,
        AssertionStoreReadParams,
        AssertionStoreReader,
    },
    ExecutorError,
};
use serde::{
    Deserialize,
    Serialize,
};

use jsonrpsee::{
    core::client::{
        ClientT,
        Error as ClientError,
    },
    http_client::{
        HttpClient,
        HttpClientBuilder,
    },
};

use tracing::{
    error,
    instrument,
};

use revm::db::EmptyDB;

use alloy_provider::{
    Provider,
    RootProvider,
};
use alloy_pubsub::PubSubFrontend;
use alloy_rpc_types::{
    BlockId,
    BlockNumHash,
    BlockNumberOrTag,
    BlockTransactionsKind,
    Filter,
    Header,
};
use alloy_sol_types::{
    sol,
    SolEvent,
};
use alloy_transport::TransportError;
use std::{
    collections::{
        BTreeMap,
        HashMap,
        HashSet,
    },
    sync::Mutex,
};
use tokio::{
    select,
    sync::{
        broadcast::error::RecvError,
        mpsc,
    },
    task::JoinHandle,
};

sol! {
    #[derive(Debug)]
    event AssertionAdded(address contractAddress, bytes32 assertionId, uint256 activeAtBlock);

    #[derive(Debug)]
    event AssertionRemoved(address contractAddress, bytes32 assertionId, uint256 activeAtBlock);

}

/// A pending assertion modification that has not passed the timelock.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum PendingModification {
    Add {
        fn_selectors: Vec<FixedBytes<4>>,
        code: Bytes,
        code_hash: B256,
        active_at_block: u64,
        log_index: u64,
    },
    Remove {
        assertion_id: B256,
        active_at_block: u64,
        log_index: u64,
    },
}

impl PendingModification {
    /// Returns the active block number for the modification event
    pub fn active_at_block(&self) -> u64 {
        match self {
            PendingModification::Add {
                active_at_block, ..
            } => *active_at_block,
            PendingModification::Remove {
                active_at_block, ..
            } => *active_at_block,
        }
    }
}

/// An active assertion contract
/// Not using `AssertionContract` for serialization purposes
#[derive(Debug, Serialize, Deserialize)]
pub struct ActiveAssertion {
    fn_selectors: Vec<FixedBytes<4>>,
    code: Bytes,
    code_hash: B256,
}

// TODO: Modify pending requests behavior, compute active at assertions for the requested block and
// cache, if the previous block has not been indexed yet.

/// An assertion store that is populated via event indexing over rpc
#[derive(Debug)]
pub struct RpcStore<P> {
    /// Rpc provider
    provider: P,
    /// Address of the state oracle contract
    state_oracle: Address,
    /// Sled database
    db: Mutex<sled::Db>,
    /// Extractor for assertion contracts from bytecode
    assertion_contract_extractor: AssertionContractExtractor,
    /// Reader for the store
    reader: AssertionStoreReader,
    /// Map of pending read requests, indexed by the block number that must be indexed before
    /// responding to the read request.
    pending_read_reqs: HashMap<u64, Vec<AssertionStoreReadParams>>,
    /// Channel for receiving read requests, Option so that it can be taken
    rx: Option<mpsc::Receiver<AssertionStoreReadParams>>,
    da_client: HttpClient,
}

#[derive(Debug, thiserror::Error)]
pub enum RpcStoreError {
    #[error("Transport error")]
    TransportError(#[from] TransportError),
    #[error("Sled error")]
    SledError(#[from] std::io::Error),
    #[error("Bincode error")]
    BincodeError(#[from] bincode::Error),
    #[error("Event decoding error")]
    EventDecodeError(#[from] alloy_sol_types::Error),
    #[error("Block number missing")]
    BlockNumberMissing,
    #[error("Log index missing")]
    LogIndexMissing,
    #[error("Block number exceeds u64")]
    BlockNumberExceedsU64,
    #[error("Assertion executor error")]
    ExecutorError(#[from] ExecutorError<<EmptyDB as DatabaseRef>::Error>),
    #[error("DA Client Error")]
    DaClientError(#[from] ClientError),
    #[error("Error decoding da bytecode")]
    DaBytecodeDecodingFailed(#[from] alloy::hex::FromHexError),
    #[error("Block stream error")]
    BlockStreamError(#[from] RecvError),
    #[error("Parent block not found")]
    ParentBlockNotFound,
    #[error("Block hash missing")]
    BlockHashMissing,
    #[error("No common ancestor found")]
    NoCommonAncestor,
    #[error("Rx is None")]
    RxNone,
    #[error("Read channel closed")]
    ReadChannelClosed,
    #[error("Failed to send read response")]
    ReadResponseFailedToSend,
}

const BLOCK_HASHES: &str = "block_hashes";
const PENDING_MODIFICATIONS: &str = "pending_modifications";
const ACTIVE_ASSERTIONS: &str = "active_assertions";

impl<P> RpcStore<P> {
    /// Creates a new `RpcStore` with the given provider, state oracle address, and sled config
    pub fn new(
        provider: P,
        state_oracle: Address,
        config: sled::Config,
        da_url: impl AsRef<str>,
        spec_id: SpecId,
        chain_id: u64,
    ) -> Result<Self, RpcStoreError> {
        let (tx, rx) = mpsc::channel(1_000);

        let reader = AssertionStoreReader::new(tx, std::time::Duration::from_secs(15));

        let da_client = HttpClientBuilder::default().build(da_url)?;

        Ok(RpcStore {
            provider,
            state_oracle,
            assertion_contract_extractor: AssertionContractExtractor::new(spec_id, chain_id),
            db: Mutex::new(config.open()?),
            rx: Some(rx),
            pending_read_reqs: HashMap::new(),
            reader,
            da_client,
        })
    }

    /// Gets a reader for the store
    pub fn get_reader(&self) -> AssertionStoreReader {
        self.reader.clone()
    }
}

impl RpcStore<RootProvider<PubSubFrontend>> {
    /// Creates a new `RpcStore` with the given provider and state oracle address
    pub async fn spawn(mut self) -> Result<JoinHandle<Result<(), RpcStoreError>>, RpcStoreError> {
        self.sync_to_finalized().await?;

        let mut rx = self.rx.take().ok_or(RpcStoreError::RxNone)?;

        Ok(tokio::spawn(async move {
            let mut stream = self.provider.subscribe_blocks().await?;
            loop {
                select! {
                    read_params = rx.recv() => { self.handle_read_req(read_params.ok_or(RpcStoreError::ReadChannelClosed)?)?; },
                    result = self.stream_blocks(&mut stream) =>  {
                        result?;
                        if !self.pending_read_reqs.is_empty() {
                            self.handle_pending_read_reqs()?;
                        }
                    },

                }
            }
        }))
    }

    /// Handles pending read requests that have been blocked by an unprocessed block number
    #[instrument(skip(self))]
    fn handle_pending_read_reqs(&mut self) -> Result<(), RpcStoreError> {
        let block_num = self
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open_tree(BLOCK_HASHES)?
            .last()?
            .map(|(last_block_num, _)| bincode::deserialize::<u64>(&last_block_num).unwrap_or(0))
            .unwrap_or_default();
        if let Some(read_requests) = self.pending_read_reqs.remove(&(block_num + 1)) {
            for read_param in read_requests {
                self.handle_read_req(read_param)?;
            }
        }
        Ok(())
    }

    /// Handles a read request by matching the traces against the active assertions, if the
    /// requested block has been indexed. If the block has not been indexed, the request is added
    /// to the pending read requests.
    #[instrument(skip(self))]
    fn handle_read_req(
        &mut self,
        read_params: AssertionStoreReadParams,
    ) -> Result<(), RpcStoreError> {
        let block_num = read_params
            .block_num
            .try_into()
            .map_err(|_| RpcStoreError::BlockNumberExceedsU64)?;

        let last_indexed_block_num = self
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open_tree(BLOCK_HASHES)?
            .last()?
            .map(|(last_block_num, _)| bincode::deserialize::<u64>(&last_block_num).unwrap_or(0))
            .unwrap_or_default();

        //If blocked add to pending
        if block_num != last_indexed_block_num + 1 {
            self.pending_read_reqs
                .entry(block_num)
                .or_default()
                .push(read_params);

            return Ok(());
        }

        //Otherwise match traces against active assertions
        let active_assertions_tree = self
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open_tree(ACTIVE_ASSERTIONS)?;

        let mut assertion_contracts = HashSet::new();
        for address in read_params.traces.calls.iter() {
            let ser_address = bincode::serialize(address)?;
            if let Some(ser_active_assertions) = active_assertions_tree.get(&ser_address)? {
                let active_assertions: Vec<ActiveAssertion> =
                    bincode::deserialize(&ser_active_assertions)?;

                let assertion_contracts_for_address = active_assertions.into_iter().map(
                    |ActiveAssertion {
                         code,
                         code_hash,
                         fn_selectors,
                     }| {
                        AssertionContract {
                            code: Bytecode::LegacyRaw(code),
                            code_hash,
                            fn_selectors,
                        }
                    },
                );

                assertion_contracts.extend(assertion_contracts_for_address);
            }
        }
        read_params
            .resp_tx
            .send(Vec::from_iter(assertion_contracts))
            .map_err(|_| RpcStoreError::ReadResponseFailedToSend)?;

        Ok(())
    }

    /// Syncs the store to the finalized block.
    #[instrument(skip(self))]
    async fn sync_to_finalized(&self) -> Result<(), RpcStoreError> {
        let finalized_block = match self
            .provider
            .get_block_by_number(BlockNumberOrTag::Finalized, BlockTransactionsKind::Hashes)
            .await?
        {
            Some(block) => block,
            None => return Ok(()),
        };

        // Get the point from which to start indexing
        // If the store is empty, start from block 0
        // Otherwise, start from the
        let from = match self
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open_tree(BLOCK_HASHES)?
            .last()?
        {
            Some((block_number, _)) => bincode::deserialize::<u64>(&block_number)? + 1_u64,
            None => 0_u64,
        };

        let finalized_block_number = finalized_block.header.inner.number;
        let finalized_block_hash = finalized_block.header.hash;

        self.index_range(from, finalized_block_number, false)
            .await?;
        self.apply_pending_modifications(finalized_block_number, finalized_block_hash)
            .await?;

        Ok(())
    }

    /// Streams blocks from the provider and indexes the events from the State Oracle contract.
    /// If the new blocks parent block hash is not the same as the last indexed block hash, then
    /// check if the latest indexed block has been reorged out.
    /// If the latest indexed block has been reorged, handle the reorg
    /// then index the range from the last indexed block to the new block
    #[instrument(skip(self, stream))]
    async fn stream_blocks(
        &self,
        stream: &mut alloy_pubsub::Subscription<Header>,
    ) -> Result<(), RpcStoreError> {
        let block = stream.recv().await?;
        let block_number = block.inner.number;

        let parent_block_hash = block.parent_hash;

        let last_indexed_block = self
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open_tree(BLOCK_HASHES)?
            .last()?;

        let mut from_block = 0;

        if let Some((last_indexed_block_num, last_indexed_block_hash)) = last_indexed_block {
            let last_indexed_block = BlockNumHash {
                number: bincode::deserialize::<u64>(&last_indexed_block_num)?,
                hash: bincode::deserialize::<B256>(&last_indexed_block_hash)?,
            };

            //Check if new block is part of the same chain
            let is_reorg = self.check_if_reorged(&block, last_indexed_block).await?;
            // If not, find the common ancestor, and remove pending modifications and block
            if is_reorg {
                let common_ancestor = self.find_common_ancestor(parent_block_hash).await?;

                let db_guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
                let block_hashes_tree = db_guard.open_tree(BLOCK_HASHES)?;
                let pending_modifications_tree = db_guard.open_tree(PENDING_MODIFICATIONS)?;

                // Remove block hash and pending modifications from the common ancestor + 1 to the
                // last indexed block
                let start = bincode::serialize(&(common_ancestor + 1))?;
                let end = bincode::serialize(&last_indexed_block.number)?;

                while let Some((key, _)) =
                    block_hashes_tree.pop_first_in_range(start.clone()..=end.clone())?
                {
                    pending_modifications_tree.remove(&key)?;
                }

                from_block = common_ancestor;
            } else {
                from_block = last_indexed_block.number;
            }
        }

        // index the range from the last indexed block to the new block
        self.index_range(from_block, block_number, true).await?;
        self.apply_pending_modifications(block_number, block.hash)
            .await?;

        // prune historical blocks every 100 blocks
        if block_number % 100 == 0 {
            if let Some(finalized_block) = self
                .provider
                .get_block_by_number(BlockNumberOrTag::Finalized, BlockTransactionsKind::Hashes)
                .await?
            {
                self.prune_finalized_block_hashes(finalized_block.header.inner.number)
                    .await?;
            }
        }
        Ok(())
    }

    /// Checks if the new block is part of the same chain as the last indexed block.
    #[instrument(skip(self))]
    async fn check_if_reorged(
        &self,
        new_block: &Header,
        last_indexed_block: BlockNumHash,
    ) -> Result<bool, RpcStoreError> {
        // If the new block number is the same as the last indexed block number, then
        // block is part of the same chain
        if new_block.number == last_indexed_block.number {
            return Ok(last_indexed_block.hash != new_block.hash);
        }

        // Traverse parent blocks from the new block until the parent blocks number is equal to
        // the last indexed block number.
        let mut cursor_hash = new_block.parent_hash;
        loop {
            let cursor = self
                .provider
                .get_block_by_hash(cursor_hash, BlockTransactionsKind::Hashes)
                .await?
                .ok_or(RpcStoreError::ParentBlockNotFound)?;

            cursor_hash = cursor.header.parent_hash;

            // Continue if the parent block
            if cursor.header.number != last_indexed_block.number {
                continue;
            }

            return Ok(cursor.header.hash != last_indexed_block.hash);
        }
    }

    /// Finds the common ancestor
    /// Traverses from the cursor backwords until it finds a common ancestor in the block_hashes
    /// tree.
    #[instrument(skip(self))]
    async fn find_common_ancestor(&self, cursor_hash: B256) -> Result<u64, RpcStoreError> {
        let block_hashes_tree = self
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open_tree(BLOCK_HASHES)?;

        let mut cursor_hash = cursor_hash;
        loop {
            let cursor = self
                .provider
                .get_block_by_hash(cursor_hash, BlockTransactionsKind::Hashes)
                .await?
                .ok_or(RpcStoreError::ParentBlockNotFound)?;

            cursor_hash = cursor.header.parent_hash;
            if let Some(hash) = block_hashes_tree.get(bincode::serialize(&cursor.header.number)?)? {
                if hash == bincode::serialize(&cursor.header.hash)? {
                    return Ok(cursor.header.number);
                }
            } else {
                return Err(RpcStoreError::NoCommonAncestor);
            }
        }
    }

    /// Applies the pending modifications to the active_assertions tree that have passed the timelock.
    /// Removes the events from the pending_modifications tree that have passed the timelock.
    /// Removes finalized blocks every 100 blocks.
    #[instrument(skip(self))]
    async fn apply_pending_modifications(
        &self,
        latest_block: u64,
        latest_block_hash: B256,
    ) -> Result<(), RpcStoreError> {
        let mut active_assertions_batch = sled::Batch::default();

        let db_guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let pending_modifications_tree = db_guard.open_tree(PENDING_MODIFICATIONS)?;

        while let Some((key, value)) = pending_modifications_tree.first()? {
            let pending_modifications: Vec<PendingModification> = bincode::deserialize(&value)?;

            // All pending modifications should have the same active block number since they were
            // logged in the same block
            // If the first modification is not active yet, we can break out of the loop
            if let Some(first_modification) = pending_modifications.first() {
                if first_modification.active_at_block() > latest_block {
                    break;
                }
            }

            pending_modifications_tree.remove(&key)?;

            for pending_modification in pending_modifications {
                match pending_modification {
                    PendingModification::Add {
                        fn_selectors,
                        code,
                        code_hash,
                        ..
                    } => {
                        active_assertions_batch.insert(
                            bincode::serialize(&code_hash)?,
                            bincode::serialize(&ActiveAssertion {
                                fn_selectors,
                                code,
                                code_hash,
                            })?,
                        );
                    }
                    PendingModification::Remove { assertion_id, .. } => {
                        active_assertions_batch.remove(bincode::serialize(&assertion_id)?);
                    }
                }
            }
        }

        db_guard
            .open_tree(ACTIVE_ASSERTIONS)?
            .apply_batch(active_assertions_batch)?;

        Ok(())
    }

    /// Prunes the finalized block hashes from the block_hashes tree.
    /// Used when streaming blocks.
    async fn prune_finalized_block_hashes(
        &self,
        finalized_block_number: u64,
    ) -> Result<(), RpcStoreError> {
        let start = bincode::serialize(&0)?;
        let end = bincode::serialize(&finalized_block_number)?;
        let db_guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let block_hashes_tree = db_guard.open_tree(BLOCK_HASHES)?;

        while block_hashes_tree
            .pop_first_in_range(start.clone()..=end.clone())?
            .is_some()
        {}

        Ok(())
    }

    /// Fetch the events from the State Oracle contract.
    /// Store the events in the pending_modifications tree for the indexed blocks.
    #[instrument(skip(self))]
    async fn index_range(
        &self,
        from: u64,
        to: u64,
        do_populate_block_hashes: bool,
    ) -> Result<(), RpcStoreError> {
        let filter = Filter::new()
            .address(self.state_oracle)
            .from_block(BlockNumberOrTag::Number(from))
            .to_block(BlockNumberOrTag::Number(to));

        let logs = self.provider.get_logs(&filter).await?;

        // For ordered insertion of pending modifications
        let mut pending_modifications: BTreeMap<u64, BTreeMap<u64, PendingModification>> =
            BTreeMap::new();

        for log in logs {
            let log_index = log.log_index.ok_or(RpcStoreError::LogIndexMissing)?;
            let block_number = log.block_number.ok_or(RpcStoreError::BlockNumberMissing)?;

            let modification_opt = match log.topic0() {
                Some(&AssertionAdded::SIGNATURE_HASH) => {
                    let event = log.log_decode::<AssertionAdded>()?;

                    let bytecode = self
                        .fetch_assertion_bytecode(event.inner.assertionId)
                        .await?;

                    let assertion_contract_res = self
                        .assertion_contract_extractor
                        .extract_assertion_contract(Bytecode::LegacyRaw(bytecode.clone()));

                    match assertion_contract_res {
                        Ok(assertion_contract) => {
                            let active_at_block = event
                                .inner
                                .activeAtBlock
                                .try_into()
                                .map_err(|_| RpcStoreError::BlockNumberExceedsU64)?;

                            Some(PendingModification::Add {
                                active_at_block,
                                log_index,
                                fn_selectors: assertion_contract.fn_selectors,
                                code: bytecode,
                                code_hash: event.inner.assertionId,
                            })
                        }
                        Err(e) => {
                            error!("Failed to extract assertion contract: {:?}", e);
                            None
                        }
                    }
                }

                Some(&AssertionRemoved::SIGNATURE_HASH) => {
                    let event = log.log_decode::<AssertionRemoved>()?;

                    let active_at_block = event
                        .inner
                        .activeAtBlock
                        .try_into()
                        .map_err(|_| RpcStoreError::BlockNumberExceedsU64)?;

                    Some(PendingModification::Remove {
                        assertion_id: event.inner.assertionId,
                        active_at_block,
                        log_index,
                    })
                }
                _ => None,
            };

            if let Some(modification) = modification_opt {
                pending_modifications
                    .entry(block_number)
                    .or_default()
                    .insert(log_index, modification);
            }
        }

        let mut pending_mods_batch = sled::Batch::default();

        for (block, log_map) in pending_modifications.iter() {
            let block_mods = log_map.values().collect::<Vec<&PendingModification>>();
            pending_mods_batch.insert(bincode::serialize(block)?, bincode::serialize(&block_mods)?);
        }
        let block_hashes_tree = {
            let db_guard = self.db.lock().unwrap_or_else(|e| e.into_inner());

            db_guard
                .open_tree(PENDING_MODIFICATIONS)?
                .apply_batch(pending_mods_batch)?;

            db_guard.open_tree(BLOCK_HASHES)
        }?;

        if do_populate_block_hashes {
            let mut block_hashes_batch = sled::Batch::default();
            for i in from..=to {
                let block_hash = self
                    .provider
                    .get_block(
                        BlockId::Number(BlockNumberOrTag::Number(i)),
                        BlockTransactionsKind::Hashes,
                    )
                    .await?
                    .ok_or(RpcStoreError::BlockHashMissing)?
                    .header
                    .hash;

                block_hashes_batch
                    .insert(bincode::serialize(&i)?, bincode::serialize(&block_hash)?);
            }
            block_hashes_tree.apply_batch(block_hashes_batch)?;
        }

        Ok(())
    }

    /// Fetch the bytecode for the given assertion id from the DA layer
    async fn fetch_assertion_bytecode(&self, assertion_id: B256) -> Result<Bytes, RpcStoreError> {
        let code = self
            .da_client
            .request::<Bytes, &[String]>("da_get_assertion", &[assertion_id.to_string()])
            .await?;

        Ok(code)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        inspectors::tracer::CallTracer,
        primitives::{
            Address,
            U256,
        },
        test_utils::{
            deployed_bytecode,
            selector_assertion,
        },
    };

    use sled::Config;

    use alloy::hex::FromHex;
    use alloy_network::{
        EthereumWallet,
        TransactionBuilder,
    };
    use alloy_node_bindings::{
        Anvil,
        AnvilInstance,
    };
    use alloy_provider::{
        ext::AnvilApi,
        ProviderBuilder,
    };
    use alloy_rpc_types::TransactionRequest;
    use alloy_rpc_types_anvil::MineOptions;
    use alloy_signer_local::PrivateKeySigner;
    use alloy_sol_types::SolCall;
    use alloy_transport_ws::WsConnect;

    use std::net::SocketAddr;

    use jsonrpsee::{
        server::{
            RpcModule,
            ServerBuilder,
            ServerHandle,
        },
        types::ErrorObjectOwned,
    };

    use tokio::sync::oneshot;

    sol! {

        function addAssertion(address contractAddress, bytes32 assertionId) public {}

        function removeAssertion(address contractAddress, bytes32 assertionId) public {}

    }

    async fn setup() -> (
        RpcStore<RootProvider<PubSubFrontend>>,
        ServerHandle,
        AnvilInstance,
    ) {
        let anvil = Anvil::new().spawn();
        let provider = ProviderBuilder::new()
            .on_ws(WsConnect::new(anvil.ws_endpoint()))
            .await
            .unwrap();

        provider.anvil_set_auto_mine(false).await.unwrap();
        let state_oracle = Address::new([0; 20]);
        let config = Config::tmp().unwrap();
        let (handle, port) = mock_da_server().await;

        (
            RpcStore::new(
                provider,
                state_oracle,
                config,
                format!("http://127.0.0.1:{port}"),
                SpecId::LATEST,
                1,
            )
            .unwrap(),
            handle,
            anvil,
        )
    }

    async fn mock_da_server() -> (ServerHandle, u16) {
        let server = ServerBuilder::default()
            .build(format!("127.0.0.1:0").parse::<SocketAddr>().unwrap())
            .await
            .unwrap();

        let da = std::sync::Mutex::new(std::collections::HashMap::<B256, Bytes>::new());

        let mut module = RpcModule::new(da);
        module
            .register_method("da_get_assertion", |params, ctx, _| {
                let string: String = params.one()?;

                let b256 = B256::from_hex(string).map_err(|_| {
                    ErrorObjectOwned::owned(500, "Failed to decode hex of id", None::<()>)
                })?;

                match ctx.lock().unwrap_or_else(|e| e.into_inner()).get(&b256) {
                    Some(code) => Ok(code.to_string()),
                    None => {
                        Err(ErrorObjectOwned::owned(
                            404,
                            "Assertion not found".to_string(),
                            None::<()>,
                        ))
                    }
                }
            })
            .unwrap();

        module
            .register_method("da_submit_assertion", |params, ctx, _| {
                let (id_string, code_string): (String, String) = params.parse()?;

                let b256 = B256::from_hex(id_string).map_err(|_| {
                    ErrorObjectOwned::owned(500, "Failed to decode hex of id", None::<()>)
                })?;
                let code = Bytes::from_hex(code_string).map_err(|_| {
                    ErrorObjectOwned::owned(500, "Failed to decode hex of code", None::<()>)
                })?;

                ctx.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(b256, code);
                Ok::<(), ErrorObjectOwned>(())
            })
            .unwrap();

        let port = server.local_addr().unwrap().port();
        let handle = server.start(module);

        (handle, port)
    }

    async fn submit_assertion<P>(
        store: &RpcStore<P>,
        assertion_id: B256,
        code: Bytes,
    ) -> Result<(), RpcStoreError> {
        store
            .da_client
            .request::<(), (String, String)>(
                "da_submit_assertion",
                (assertion_id.to_string(), code.to_string()),
            )
            .await?;
        Ok(())
    }

    async fn mine_block_write_hash(rpc_store: &RpcStore<RootProvider<PubSubFrontend>>) -> Header {
        let header = mine_block(rpc_store).await;

        rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap()
            .insert(
                bincode::serialize(&header.number).unwrap(),
                bincode::serialize(&header.hash).unwrap(),
            )
            .unwrap();

        header
    }

    async fn mine_block(rpc_store: &RpcStore<RootProvider<PubSubFrontend>>) -> Header {
        let _ = rpc_store.provider.evm_mine(None).await;
        let block = rpc_store
            .provider
            .get_block_by_number(Default::default(), Default::default())
            .await
            .unwrap()
            .unwrap();

        block.header
    }

    #[tokio::test]
    async fn test_apply_pending_modifications() {
        let (rpc_store, _da, _anvil) = setup().await;

        let pending_modifications_tree = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(PENDING_MODIFICATIONS)
            .unwrap();
        let active_assertions_tree = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(ACTIVE_ASSERTIONS)
            .unwrap();

        for (index, modification) in vec![
            PendingModification::Add {
                fn_selectors: vec![FixedBytes::default()],
                code: Bytes::default(),
                code_hash: B256::from([0; 32]),
                active_at_block: 0,
                log_index: 1,
            },
            PendingModification::Remove {
                assertion_id: B256::from([0; 32]),
                active_at_block: 1,
                log_index: 1,
            },
        ]
        .iter()
        .enumerate()
        {
            pending_modifications_tree
                .insert(
                    bincode::serialize(&(index as u64)).unwrap(),
                    bincode::serialize(&vec![modification]).unwrap(),
                )
                .unwrap();
        }

        assert_eq!(active_assertions_tree.len(), 0);
        assert_eq!(pending_modifications_tree.len(), 2);

        let b_hash_0 = B256::from([0; 32]);
        rpc_store
            .apply_pending_modifications(0, b_hash_0)
            .await
            .unwrap();

        assert_eq!(active_assertions_tree.len(), 1);
        assert_eq!(
            active_assertions_tree
                .get(bincode::serialize(&b_hash_0).unwrap())
                .unwrap()
                .unwrap(),
            bincode::serialize(&ActiveAssertion {
                fn_selectors: vec![FixedBytes::default()],
                code: Bytes::default(),
                code_hash: B256::from([0; 32]),
            })
            .unwrap()
        );

        assert_eq!(pending_modifications_tree.len(), 1);
        assert_eq!(
            pending_modifications_tree
                .get(bincode::serialize(&0).unwrap())
                .unwrap(),
            None
        );

        let b_hash_1 = B256::from([1; 32]);
        rpc_store
            .apply_pending_modifications(1, b_hash_1)
            .await
            .unwrap();

        assert_eq!(active_assertions_tree.len(), 0);
        assert_eq!(pending_modifications_tree.len(), 0);
    }

    #[tokio::test]
    async fn test_prune_finalized_block_hashes() {
        let (rpc_store, _da, _anvil) = setup().await;

        let block_hash_tree = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap();

        for i in 0..100 {
            block_hash_tree
                .insert(
                    bincode::serialize(&(i as u64)).unwrap(),
                    bincode::serialize(&B256::default()).unwrap(),
                )
                .unwrap();
        }

        assert_eq!(block_hash_tree.len(), 100);

        rpc_store.prune_finalized_block_hashes(0).await.unwrap();

        assert_eq!(block_hash_tree.len(), 99);
        assert_eq!(
            block_hash_tree
                .get(bincode::serialize(&0).unwrap())
                .unwrap(),
            None
        );

        rpc_store.prune_finalized_block_hashes(1).await.unwrap();
        assert_eq!(block_hash_tree.len(), 98);
        assert_eq!(
            block_hash_tree
                .get(bincode::serialize(&1).unwrap())
                .unwrap(),
            None
        );

        rpc_store.prune_finalized_block_hashes(100).await.unwrap();
        assert_eq!(block_hash_tree.len(), 0);
    }

    #[tokio::test]
    async fn test_index_range() {
        let (rpc_store, _da, _anvil) = setup().await;

        let registration_mock = deployed_bytecode("AssertionRegistration.sol:RegistrationMock");
        let chain_id = rpc_store.provider.get_chain_id().await.unwrap();

        rpc_store
            .provider
            .anvil_set_code(rpc_store.state_oracle, registration_mock)
            .await
            .unwrap();
        let signer = PrivateKeySigner::random();
        let wallet = EthereumWallet::from(signer.clone());

        let protocol_addr = Address::random();

        let selector_assertion = selector_assertion();

        let assertion_id = selector_assertion.code_hash;

        let assertion = selector_assertion.code.original_bytes();
        submit_assertion(&rpc_store, assertion_id, assertion.clone().into())
            .await
            .unwrap();

        rpc_store
            .provider
            .anvil_set_balance(signer.address(), U256::MAX)
            .await
            .unwrap();

        for (nonce, input) in vec![
            (
                0,
                addAssertionCall::new((protocol_addr, assertion_id)).abi_encode(),
            ),
            (
                1,
                removeAssertionCall::new((protocol_addr, assertion_id)).abi_encode(),
            ),
        ] {
            let tx = TransactionRequest::default()
                .with_to(rpc_store.state_oracle)
                .with_input(input)
                .with_nonce(nonce)
                .with_chain_id(chain_id)
                .with_gas_limit(25_000)
                .with_max_priority_fee_per_gas(1_000_000_000)
                .with_max_fee_per_gas(20_000_000_000);

            let tx_envelope = tx.build(&wallet).await.unwrap();

            let _ = rpc_store
                .provider
                .send_tx_envelope(tx_envelope)
                .await
                .unwrap();
        }

        let block = rpc_store
            .provider
            .get_block_by_number(0.into(), Default::default())
            .await
            .unwrap()
            .unwrap();
        let block_number = block.header.number;

        assert_eq!(block_number, 0);

        let timestamp = block.header.inner.timestamp;

        let _ = rpc_store
            .provider
            .evm_mine(Some(MineOptions::Timestamp(Some(timestamp + 5))))
            .await;

        rpc_store.index_range(0, 1, true).await.unwrap();

        let pending_mods_tree = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(PENDING_MODIFICATIONS)
            .unwrap();
        assert_eq!(pending_mods_tree.len(), 1);

        let key = bincode::serialize(&1_u64).unwrap();
        let value = pending_mods_tree.get(&key).unwrap().unwrap();

        let pending_modifications: Vec<PendingModification> = bincode::deserialize(&value).unwrap();

        assert_eq!(
            pending_modifications,
            vec![
                PendingModification::Add {
                    active_at_block: 65,
                    log_index: 0,
                    code_hash: assertion_id,
                    code: assertion,
                    fn_selectors: selector_assertion.fn_selectors
                },
                PendingModification::Remove {
                    active_at_block: 65,
                    log_index: 1,
                    assertion_id,
                },
            ]
        );
    }

    #[tokio::test]
    async fn test_check_if_reorged_no_reorg() {
        let (rpc_store, _da, _anvil) = setup().await;

        // Mine initial block
        let block0 = rpc_store
            .provider
            .get_block_by_number(0.into(), Default::default())
            .await
            .unwrap()
            .unwrap();

        // Store initial block in db
        rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap()
            .insert(
                bincode::serialize(&0u64).unwrap(),
                bincode::serialize(&block0.header.hash).unwrap(),
            )
            .unwrap();

        // Mine next block
        let block1 = mine_block_write_hash(&rpc_store).await;

        // Check if reorged - should be false since blocks are in sequence
        let is_reorged = rpc_store
            .check_if_reorged(
                &block1,
                BlockNumHash {
                    number: 0,
                    hash: block0.header.hash,
                },
            )
            .await
            .unwrap();

        assert!(!is_reorged);
    }

    #[tokio::test]
    async fn test_check_if_reorged_with_reorg() {
        let (rpc_store, _da, _anvil) = setup().await;

        let block0 = rpc_store
            .provider
            .get_block_by_number(0.into(), Default::default())
            .await
            .unwrap()
            .unwrap();

        // Store it in db
        rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap()
            .insert(
                bincode::serialize(&0u64).unwrap(),
                bincode::serialize(&block0.header.hash).unwrap(),
            )
            .unwrap();

        let block1 = mine_block_write_hash(&rpc_store).await;

        // Simulate reorg by modifying anvil state
        let reorg_options = alloy_rpc_types_anvil::ReorgOptions {
            depth: 1,
            tx_block_pairs: vec![],
        };
        rpc_store.provider.anvil_reorg(reorg_options).await.unwrap();

        // Mine new block on different fork
        let new_block1 = mine_block_write_hash(&rpc_store).await;

        // Check if reorged - should be true since we created a new fork
        let is_reorged = rpc_store
            .check_if_reorged(
                &new_block1,
                BlockNumHash {
                    number: 1,
                    hash: block1.hash,
                },
            )
            .await
            .unwrap();

        assert!(is_reorged);
    }

    #[tokio::test]
    async fn test_find_common_ancestor_shallow_reorg() {
        let (rpc_store, _da, _anvil) = setup().await;

        // Mine and store initial blocks
        let _ = mine_block_write_hash(&rpc_store).await;
        let _ = mine_block_write_hash(&rpc_store).await;
        let _ = mine_block_write_hash(&rpc_store).await;

        // Create reorg by resetting to block 1

        let reorg_options = alloy_rpc_types_anvil::ReorgOptions {
            depth: 1,
            tx_block_pairs: vec![],
        };

        rpc_store.provider.anvil_reorg(reorg_options).await.unwrap();

        // Mine new block on different fork
        let _ = rpc_store.provider.evm_mine(None).await;

        let new_block = rpc_store
            .provider
            .get_block_by_number(2.into(), Default::default())
            .await
            .unwrap()
            .unwrap();

        // Find common ancestor
        let common_ancestor = rpc_store
            .find_common_ancestor(new_block.header.inner.parent_hash)
            .await
            .unwrap();

        // Common ancestor should be block 1
        assert_eq!(common_ancestor, 1);
    }

    #[tokio::test]
    async fn test_find_common_ancestor_missing_blocks() {
        let (rpc_store, _da, _anvil) = setup().await;

        // Mine block but don't store it
        let _ = mine_block(&rpc_store).await;

        // Create reorg
        let reorg_options = alloy_rpc_types_anvil::ReorgOptions {
            depth: 1,
            tx_block_pairs: vec![],
        };
        rpc_store.provider.anvil_reorg(reorg_options).await.unwrap();

        let new_block1 = mine_block(&rpc_store).await;

        // Should fail to find common ancestor since no blocks are stored
        let result = rpc_store
            .find_common_ancestor(new_block1.inner.parent_hash)
            .await;

        assert!(matches!(result, Err(RpcStoreError::NoCommonAncestor)));
    }
    #[tokio::test]
    async fn test_stream_blocks_normal_case() {
        let (rpc_store, _da, _anvil) = setup().await;

        // Set up initial state
        let block0 = rpc_store
            .provider
            .get_block_by_number(0.into(), Default::default())
            .await
            .unwrap()
            .unwrap();

        // Store initial block in db
        rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap()
            .insert(
                bincode::serialize(&0u64).unwrap(),
                bincode::serialize(&block0.header.hash).unwrap(),
            )
            .unwrap();

        // Create a block subscription
        let mut block_stream = rpc_store.provider.subscribe_blocks().await.unwrap();

        // Mine a new block
        let _ = rpc_store.provider.evm_mine(None).await;

        // Process the new block
        rpc_store.stream_blocks(&mut block_stream).await.unwrap();

        // Verify block was properly indexed
        let block_hashes = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap();

        // Should have both blocks (0 and 1)
        assert_eq!(block_hashes.len(), 2);
    }

    #[tokio::test]
    async fn test_stream_blocks_with_reorg() {
        let (rpc_store, _da, _anvil) = setup().await;

        // Set up initial chain
        let block0 = rpc_store
            .provider
            .get_block_by_number(0.into(), Default::default())
            .await
            .unwrap()
            .unwrap();

        // Store blocks in db
        let block_hashes = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap();

        block_hashes
            .insert(
                bincode::serialize(&0u64).unwrap(),
                bincode::serialize(&block0.header.hash).unwrap(),
            )
            .unwrap();

        // Mine block 1
        let block1 = mine_block_write_hash(&rpc_store).await;

        // Create a block subscription
        let mut block_stream = rpc_store.provider.subscribe_blocks().await.unwrap();

        // Simulate a reorg
        let reorg_options = alloy_rpc_types_anvil::ReorgOptions {
            depth: 1,
            tx_block_pairs: vec![],
        };
        rpc_store.provider.anvil_reorg(reorg_options).await.unwrap();

        // Process the newly mined block
        rpc_store.stream_blocks(&mut block_stream).await.unwrap();

        // Verify state after reorg
        let latest_block = block_hashes.last().unwrap().unwrap();

        let latest_hash: B256 = bincode::deserialize(&latest_block.1).unwrap();

        // Hash should be different from original block1
        assert_ne!(latest_hash, block1.hash);

        // Should still have 2 blocks but block1 should be different
        assert_eq!(block_hashes.len(), 2, "Block hashes len");
    }

    #[tokio::test]
    async fn test_stream_blocks_pruning() {
        let (rpc_store, _da, _anvil) = setup().await;

        // Mine up to block 99 and write hashes to store
        for _ in 1..=99u64 {
            mine_block_write_hash(&rpc_store).await;
        }

        let block_hashes = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap();

        assert_eq!(block_hashes.len(), 99);

        // Create subscription and mine block 100
        let mut block_stream = rpc_store.provider.subscribe_blocks().await.unwrap();

        // Mine block 100 which should trigger pruning
        let _ = rpc_store.provider.evm_mine(None).await;

        // Process the new block which should trigger pruning
        rpc_store.stream_blocks(&mut block_stream).await.unwrap();

        assert_eq!(block_hashes.len(), 64);
        assert_eq!(
            block_hashes.first().unwrap().unwrap().0,
            bincode::serialize(&37u64).unwrap()
        );
        assert_eq!(
            block_hashes.last().unwrap().unwrap().0,
            bincode::serialize(&100_u64).unwrap()
        );
    }

    #[tokio::test]
    async fn test_stream_blocks_with_pending_modifications() {
        let (rpc_store, _da, _anvil) = setup().await;

        // Add pending modification
        let pending_mods = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(PENDING_MODIFICATIONS)
            .unwrap();

        let modification = PendingModification::Add {
            fn_selectors: vec![FixedBytes::default()],
            code: Bytes::default(),
            code_hash: B256::default(),
            active_at_block: 1,
            log_index: 0,
        };

        pending_mods
            .insert(
                bincode::serialize(&0u64).unwrap(),
                bincode::serialize(&vec![modification]).unwrap(),
            )
            .unwrap();

        // Create subscription and mine block 1
        let mut block_stream = rpc_store.provider.subscribe_blocks().await.unwrap();
        let _ = rpc_store.provider.evm_mine(None).await;

        // Process block which should apply modification
        rpc_store.stream_blocks(&mut block_stream).await.unwrap();

        // Verify modification was applied
        let active_assertions = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(ACTIVE_ASSERTIONS)
            .unwrap();

        // Pending modification should be removed
        assert_eq!(pending_mods.len(), 0);

        // Active assertion should be added
        assert_eq!(active_assertions.len(), 1);
    }
    #[tokio::test]
    async fn test_handle_read_req_already_indexed() {
        let (mut rpc_store, _da, _anvil) = setup().await;

        // Create an active assertion
        let active_assertions_tree = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(ACTIVE_ASSERTIONS)
            .unwrap();

        let test_address = Address::random();
        let test_selector = FixedBytes::<4>::random();
        let test_code = Bytes::from_static(b"test code");
        let test_hash = B256::random();

        let active_assertion = ActiveAssertion {
            fn_selectors: vec![test_selector],
            code: test_code.clone(),
            code_hash: test_hash,
        };

        active_assertions_tree
            .insert(
                bincode::serialize(&test_address).unwrap(),
                bincode::serialize(&vec![active_assertion]).unwrap(),
            )
            .unwrap();

        // Set up block hash to indicate block is indexed
        let block_number = 1u64;
        let block_hash = B256::random();
        rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap()
            .insert(
                bincode::serialize(&block_number).unwrap(),
                bincode::serialize(&block_hash).unwrap(),
            )
            .unwrap();

        // Create read request
        let (tx, mut rx) = oneshot::channel();
        let read_params = AssertionStoreReadParams {
            block_num: U256::from(block_number + 1),
            traces: CallTracer {
                calls: HashSet::from_iter(vec![test_address]),
                ..Default::default()
            },
            resp_tx: tx,
        };

        // Handle read request
        rpc_store.handle_read_req(read_params).unwrap();

        // Verify response
        let response = rx.try_recv().unwrap();

        assert!(rpc_store.pending_read_reqs.is_empty());
        assert_eq!(response.len(), 1);
        assert_eq!(response[0].fn_selectors, vec![test_selector]);
        assert_eq!(response[0].code, Bytecode::LegacyRaw(test_code));
        assert_eq!(response[0].code_hash, test_hash);
    }

    #[tokio::test]
    async fn test_handle_read_req_pending() {
        let (mut rpc_store, _da, _anvil) = setup().await;

        // Create read request for future block
        let block_number = 100u64;
        let (tx, _rx) = oneshot::channel();
        let read_params = AssertionStoreReadParams {
            block_num: U256::from(block_number),
            traces: CallTracer::default(),
            resp_tx: tx,
        };

        // Handle read request - should be stored as pending
        rpc_store.handle_read_req(read_params).unwrap();

        // Verify request was stored as pending
        assert!(rpc_store.pending_read_reqs.contains_key(&block_number));
        assert_eq!(rpc_store.pending_read_reqs[&block_number].len(), 1);
    }

    #[tokio::test]
    async fn test_handle_pending_read_reqs() {
        let (mut rpc_store, _da, _anvil) = setup().await;

        // Create an active assertion
        let active_assertions_tree = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(ACTIVE_ASSERTIONS)
            .unwrap();

        let test_address = Address::random();
        let test_selector = FixedBytes::from([0x12, 0x34, 0x56, 0x78]);
        let test_code = Bytes::from_static(b"test code");
        let test_hash = B256::random();

        let active_assertion = ActiveAssertion {
            fn_selectors: vec![test_selector],
            code: test_code.clone(),
            code_hash: test_hash,
        };

        active_assertions_tree
            .insert(
                bincode::serialize(&test_address).unwrap(),
                bincode::serialize(&vec![active_assertion]).unwrap(),
            )
            .unwrap();

        // Create pending read request
        let read_req_block_number = 2u64;
        let (tx, mut rx) = oneshot::channel();
        let read_params = AssertionStoreReadParams {
            block_num: U256::from(read_req_block_number),
            traces: CallTracer {
                calls: HashSet::from_iter(vec![test_address]),
                ..Default::default()
            },
            resp_tx: tx,
        };

        // Add request to pending map
        rpc_store
            .pending_read_reqs
            .insert(read_req_block_number, vec![read_params]);

        // Add block hash to indicate block is now indexed
        let block_hash = B256::random();
        rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap()
            .insert(
                bincode::serialize(&1_u64).unwrap(),
                bincode::serialize(&block_hash).unwrap(),
            )
            .unwrap();

        // Handle pending read requests
        rpc_store.handle_pending_read_reqs().unwrap();

        // Verify pending request was processed
        assert!(rpc_store.pending_read_reqs.is_empty());

        // Verify response was sent
        let response = rx.try_recv().unwrap();
        assert_eq!(response.len(), 1);
        assert_eq!(response[0].fn_selectors, vec![test_selector]);
        assert_eq!(response[0].code, Bytecode::LegacyRaw(test_code));
        assert_eq!(response[0].code_hash, test_hash);
    }

    #[tokio::test]
    async fn test_handle_read_req_no_matching_assertions() {
        let (mut rpc_store, _da, _anvil) = setup().await;

        // Set up block hash to indicate block is indexed
        let block_number = 1u64;
        let block_hash = B256::random();
        rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap()
            .insert(
                bincode::serialize(&block_number).unwrap(),
                bincode::serialize(&block_hash).unwrap(),
            )
            .unwrap();

        // Create read request with address that has no assertions
        let (tx, mut rx) = oneshot::channel();
        let read_params = AssertionStoreReadParams {
            block_num: U256::from(block_number + 1),
            traces: CallTracer {
                calls: HashSet::from_iter(vec![Address::random()]),
                ..Default::default()
            },
            resp_tx: tx,
        };

        // Handle read request
        rpc_store.handle_read_req(read_params).unwrap();

        // Verify empty response
        let response = rx.try_recv().unwrap();
        assert!(response.is_empty());
    }

    #[tokio::test]
    async fn test_handle_read_req_multiple_assertions() {
        let (mut rpc_store, _da, _anvil) = setup().await;

        // Create multiple active assertions for same address
        let active_assertions_tree = rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(ACTIVE_ASSERTIONS)
            .unwrap();

        let test_address = Address::random();
        let assertions = vec![
            ActiveAssertion {
                fn_selectors: vec![FixedBytes::from([0x12, 0x34, 0x56, 0x78])],
                code: Bytes::from_static(b"test code 1"),
                code_hash: B256::random(),
            },
            ActiveAssertion {
                fn_selectors: vec![FixedBytes::from([0x87, 0x65, 0x43, 0x21])],
                code: Bytes::from_static(b"test code 2"),
                code_hash: B256::random(),
            },
        ];

        active_assertions_tree
            .insert(
                bincode::serialize(&test_address).unwrap(),
                bincode::serialize(&assertions).unwrap(),
            )
            .unwrap();

        // Set up block hash
        let block_number = 1u64;
        let block_hash = B256::random();
        rpc_store
            .db
            .lock()
            .unwrap()
            .open_tree(BLOCK_HASHES)
            .unwrap()
            .insert(
                bincode::serialize(&block_number).unwrap(),
                bincode::serialize(&block_hash).unwrap(),
            )
            .unwrap();

        // Create read request
        let (tx, mut rx) = oneshot::channel();
        let read_params = AssertionStoreReadParams {
            block_num: U256::from(block_number + 1),
            traces: CallTracer {
                calls: HashSet::from_iter(vec![test_address]),
                ..Default::default()
            },
            resp_tx: tx,
        };

        // Handle read request
        rpc_store.handle_read_req(read_params).unwrap();

        // Verify response contains both assertions
        let mut response = rx.try_recv().unwrap();
        response.sort_by(|a, b| a.fn_selectors[0].cmp(&b.fn_selectors[0]));

        assert_eq!(response.len(), 2);
        assert_eq!(response[0].fn_selectors, assertions[0].fn_selectors);
        assert_eq!(
            response[0].code,
            Bytecode::LegacyRaw(assertions[0].code.clone())
        );
        assert_eq!(response[0].code_hash, assertions[0].code_hash);
        assert_eq!(response[1].fn_selectors, assertions[1].fn_selectors);
        assert_eq!(
            response[1].code,
            Bytecode::LegacyRaw(assertions[1].code.clone())
        );
        assert_eq!(response[1].code_hash, assertions[1].code_hash);
    }
}
