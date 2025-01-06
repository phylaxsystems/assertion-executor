use crate::{
    db::DatabaseRef,
    primitives::{
        Address,
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

use revm::db::EmptyDB;

use alloy_provider::{
    Provider,
    RootProvider,
};
use alloy_pubsub::PubSubFrontend;
use alloy_rpc_types::{
    BlockNumberOrTag,
    BlockTransactionsKind,
    Filter,
};
use alloy_sol_types::{
    sol,
    SolEvent,
};
use alloy_transport::TransportError;
use std::{
    collections::BTreeMap,
    thread::JoinHandle,
};

use tracing::error;

use tokio::sync::mpsc;

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

/// An assertion store that is populated via event indexing over rpc
pub struct RpcStore<P> {
    provider: P,
    state_oracle: Address,
    db: sled::Db,
    assertion_contract_extractor: AssertionContractExtractor,
    reader: AssertionStoreReader,
    #[allow(dead_code)]
    rx: mpsc::Receiver<AssertionStoreReadParams>,
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
            db: config.open()?,
            assertion_contract_extractor: AssertionContractExtractor::new(spec_id, chain_id),
            rx,
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
    pub async fn spawn(&self) -> Result<JoinHandle<()>, RpcStoreError> {
        self.sync_to_finalized().await?;
        Ok(std::thread::spawn(|| {}))
    }

    /// Syncs the store to the finalized block.
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
        // Otherwise, start from the last indexed block + 1
        let from = match self.db.open_tree(BLOCK_HASHES)?.last()? {
            Some((block_number, _)) => bincode::deserialize::<u64>(&block_number)? + 1_u64,
            None => 0_u64,
        };

        let finalized_block_number = finalized_block.header.inner.number;
        let finalized_block_hash = finalized_block.header.hash;

        self.index_range(from, finalized_block_number).await?;
        self.apply_pending_modifications(finalized_block_number, finalized_block_hash)?;

        Ok(())
    }

    /// Applies the pending modifications to the active_assertions tree that have passed the timelock.
    /// Removes the events from the pending_modifications tree that have passed the timelock.
    /// Removes finalized blocks every 100 blocks.
    fn apply_pending_modifications(
        &self,
        latest_block: u64,
        latest_block_hash: B256,
    ) -> Result<(), RpcStoreError> {
        let mut active_assertions_batch = sled::Batch::default();

        let pending_modifications_tree = self.db.open_tree(PENDING_MODIFICATIONS)?;

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

        self.db
            .open_tree(ACTIVE_ASSERTIONS)?
            .apply_batch(active_assertions_batch)?;

        self.db.open_tree(BLOCK_HASHES)?.insert(
            bincode::serialize(&latest_block)?,
            bincode::serialize(&latest_block_hash)?,
        )?;

        Ok(())
    }

    /// Prunes the finalized block hashes from the block_hashes tree.
    /// Used when streaming blocks.
    #[allow(dead_code)]
    fn prune_finalized_block_hashes(
        &self,
        finalized_block_number: u64,
    ) -> Result<(), RpcStoreError> {
        let start = bincode::serialize(&0)?;
        let end = bincode::serialize(&finalized_block_number)?;

        while self
            .db
            .open_tree(BLOCK_HASHES)?
            .pop_first_in_range(start.clone()..=end.clone())?
            .is_some()
        {}

        Ok(())
    }

    /// Fetch the events from the State Oracle contract.
    /// Store the events in the pending_modifications tree for the indexed blocks.
    async fn index_range(&self, from: u64, to: u64) -> Result<(), RpcStoreError> {
        let filter = Filter::new()
            .address(self.state_oracle)
            .from_block(BlockNumberOrTag::Number(from))
            .from_block(BlockNumberOrTag::Number(to));

        let logs = self.provider.get_logs(&filter).await?;

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
            let serialized_block = bincode::serialize(block)?;
            let serialized_mods = bincode::serialize(&block_mods)?;

            pending_mods_batch.insert(serialized_block, serialized_mods);
        }

        self.db
            .open_tree(PENDING_MODIFICATIONS)?
            .apply_batch(pending_mods_batch)?;

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

    sol! {

        function addAssertion(address contractAddress, bytes32 assertionId) public {}

        function removeAssertion(address contractAddress, bytes32 assertionId) public {}

    }

    async fn setup(anvil: &AnvilInstance) -> RpcStore<RootProvider<PubSubFrontend>> {
        let provider = ProviderBuilder::new()
            .on_ws(WsConnect::new(anvil.ws_endpoint()))
            .await
            .unwrap();

        provider.anvil_set_auto_mine(false).await.unwrap();
        let state_oracle = Address::new([0; 20]);
        let config = Config::tmp().unwrap();

        RpcStore::new(
            provider,
            state_oracle,
            config,
            "http://127.0.0.1:6969",
            SpecId::LATEST,
            1,
        )
        .unwrap()
    }

    async fn mock_da_server() -> ServerHandle {
        let server = ServerBuilder::default()
            .build("127.0.0.1:6969".parse::<SocketAddr>().unwrap())
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

        server.start(module)
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

    #[tokio::test]
    async fn test_apply_pending_modifications() {
        let anvil = Anvil::new().spawn();
        let rpc_store = setup(&anvil).await;
        let pending_modifications_tree = rpc_store.db.open_tree(PENDING_MODIFICATIONS).unwrap();
        let active_assertions_tree = rpc_store.db.open_tree(ACTIVE_ASSERTIONS).unwrap();
        let block_hashes_tree = rpc_store.db.open_tree(BLOCK_HASHES).unwrap();

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
        rpc_store.apply_pending_modifications(0, b_hash_0).unwrap();

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

        assert_eq!(
            block_hashes_tree
                .get(bincode::serialize(&0_u64).unwrap())
                .unwrap()
                .unwrap(),
            bincode::serialize(&b_hash_0).unwrap()
        );

        let b_hash_1 = B256::from([1; 32]);
        rpc_store.apply_pending_modifications(1, b_hash_1).unwrap();

        assert_eq!(active_assertions_tree.len(), 0);
        assert_eq!(pending_modifications_tree.len(), 0);
        assert_eq!(
            block_hashes_tree
                .get(bincode::serialize(&1_u64).unwrap())
                .unwrap()
                .unwrap(),
            bincode::serialize(&b_hash_1).unwrap()
        );
        assert_eq!(block_hashes_tree.len(), 2);
    }

    #[tokio::test]
    async fn test_prune_finalized_block_hashes() {
        let anvil = Anvil::new().spawn();
        let rpc_store = setup(&anvil).await;
        let block_hash_tree = rpc_store.db.open_tree(BLOCK_HASHES).unwrap();

        for i in 0..100 {
            block_hash_tree
                .insert(
                    bincode::serialize(&(i as u64)).unwrap(),
                    bincode::serialize(&B256::default()).unwrap(),
                )
                .unwrap();
        }

        assert_eq!(block_hash_tree.len(), 100);

        rpc_store.prune_finalized_block_hashes(0).unwrap();

        assert_eq!(block_hash_tree.len(), 99);
        assert_eq!(
            block_hash_tree
                .get(bincode::serialize(&0).unwrap())
                .unwrap(),
            None
        );

        rpc_store.prune_finalized_block_hashes(1).unwrap();
        assert_eq!(block_hash_tree.len(), 98);
        assert_eq!(
            block_hash_tree
                .get(bincode::serialize(&1).unwrap())
                .unwrap(),
            None
        );

        rpc_store.prune_finalized_block_hashes(100).unwrap();
        assert_eq!(block_hash_tree.len(), 0);
    }

    #[tokio::test]
    async fn test_index_range() {
        let _server_handle = mock_da_server().await;
        let anvil = Anvil::new().spawn();
        let rpc_store = setup(&anvil).await;

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

        rpc_store.index_range(0, 1).await.unwrap();

        let pending_mods_tree = rpc_store.db.open_tree(PENDING_MODIFICATIONS).unwrap();
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
}
