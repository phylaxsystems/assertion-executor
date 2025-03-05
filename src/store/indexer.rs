use alloy_network::{
    BlockResponse,
    Network,
};
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
};
use alloy_transport::TransportError;

use alloy_network_primitives::HeaderResponse;

use alloy_consensus::BlockHeader;

use alloy_sol_types::{
    sol,
    SolEvent,
};

use bincode::{
    deserialize as de,
    serialize as ser,
};

use tracing::{
    error,
    instrument,
    warn,
};

use alloy::primitives::LogData;

use crate::{
    primitives::{
        Address,
        UpdateBlock,
        B256,
    },
    store::{
        extract_assertion_contract,
        AssertionStore,
        AssertionStoreError,
        PendingModification,
    },
    utils::reorg_utils::{
        check_if_reorged,
        CheckIfReorgedError,
    },
    ExecutorConfig,
};

use assertion_da_client::{
    DaClient,
    DaClientError,
};

use std::collections::BTreeMap;

sol! {
    #[derive(Debug)]
    event AssertionAdded(address contractAddress, bytes32 assertionId, uint256 activeAtBlock);

    #[derive(Debug)]
    event AssertionRemoved(address contractAddress, bytes32 assertionId, uint256 activeAtBlock);

}

/// Indexer for the State Oracle contract
/// Indexes the events emitted by the State Oracle contract
/// Writes finalized pending modifications to the Assertion Store
/// # Example
/// ``` no_run
/// use assertion_executor::{store::{AssertionStore, DaClient, Indexer, IndexerCfg}, primitives::Address,ExecutorConfig};
///
/// use sled::Config;
///
/// use alloy_network::Ethereum;
/// use alloy_provider::{ProviderBuilder, WsConnect};
///
/// #[tokio::main]
/// async fn main() {
///    let state_oracle = Address::new([0; 20]);
///    let db = Config::tmp().unwrap().open().unwrap();
///    let store = AssertionStore::new_ephemeral().unwrap();
///    let da_client = DaClient::new(&format!("http://127.0.0.1:0000")).unwrap();
///
///
///    let provider = ProviderBuilder::new()
///    .network::<Ethereum>() // Change for other networks, like Optimism.
///    .on_ws(WsConnect::new("wss://127.0.0.1:0001"))
///    .await
///    .unwrap();
///
///    let config = IndexerCfg {
///         provider,
///         block_hash_tree: db.open_tree("block_hashes").unwrap(),
///         pending_modifications_tree: db.open_tree("pending_modifications").unwrap(),
///         store,
///         da_client,
///         state_oracle,
///         executor_config: ExecutorConfig::default(),
///    };
///
///    // Await syncing the indexer to the latest block.
///    let indexer = Indexer::new_synced(config).await.unwrap();
///
///    // Streams new blocks. Awaits infinitely unless an error occurs.
///    indexer.run().await.unwrap();
/// }
pub struct Indexer<N: Network> {
    provider: PubSubProvider<N>,
    block_hash_tree: sled::Tree,
    pending_modifications_tree: sled::Tree,
    store: AssertionStore,
    da_client: DaClient,
    state_oracle: Address,
    executor_config: ExecutorConfig,
    is_synced: bool,
}

type PubSubProvider<N> = RootProvider<PubSubFrontend, N>;

#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
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
    #[error("DA Client Error")]
    DaClientError(#[from] DaClientError),
    #[error("Error decoding da bytecode")]
    DaBytecodeDecodingFailed(#[from] alloy::hex::FromHexError),
    #[error("Block stream error")]
    BlockStreamError(#[from] tokio::sync::broadcast::error::RecvError),
    #[error("Parent block not found")]
    ParentBlockNotFound,
    #[error("Block hash missing")]
    BlockHashMissing,
    #[error("No common ancestor found")]
    NoCommonAncestor,
    #[error("Execution Logs Rx is None; Channel likely dropped.")]
    ExecutionLogsRxNone,
    #[error("Check if reorged error")]
    CheckIfReorgedError(#[from] CheckIfReorgedError),
    #[error("Assertion Store Error")]
    AssertionStoreError(#[from] AssertionStoreError),
    #[error("Store must be synced before running")]
    StoreNotSynced,
}

type IndexerResult<T = ()> = std::result::Result<T, IndexerError>;

/// Configuration for the Indexer
#[derive(Debug)]
pub struct IndexerCfg<N: Network> {
    /// The State Oracle contract address
    pub state_oracle: Address,
    /// The DA Client
    pub da_client: DaClient,
    /// The Executor configuration
    pub executor_config: ExecutorConfig,
    /// The Assertion Store
    pub store: AssertionStore,
    /// Rpc Provider
    pub provider: PubSubProvider<N>,
    /// Block hash tree
    /// Used to store block hashes for reorg detection
    pub block_hash_tree: sled::Tree,
    /// Pending modifications tree
    /// Used to store pending modifications for blocks before moving
    /// them to the store
    pub pending_modifications_tree: sled::Tree,
}

impl<N: Network> Indexer<N> {
    /// Create a new Indexer
    pub fn new(cfg: IndexerCfg<N>) -> Self {
        let IndexerCfg {
            provider,
            block_hash_tree,
            pending_modifications_tree,
            store,
            da_client,
            state_oracle,
            executor_config,
        } = cfg;

        Self {
            provider,
            block_hash_tree,
            pending_modifications_tree,
            store,
            da_client,
            state_oracle,
            executor_config,
            is_synced: false,
        }
    }

    /// Create a new Indexer and sync it to the latest block
    pub async fn new_synced(cfg: IndexerCfg<N>) -> IndexerResult<Self> {
        let mut indexer = Self::new(cfg);
        indexer.sync_to_head().await?;
        Ok(indexer)
    }

    /// Run the indexer
    pub async fn run(&self) -> IndexerResult {
        if !self.is_synced {
            return Err(IndexerError::StoreNotSynced);
        }

        let mut block_stream = self.provider.subscribe_blocks().await?;
        loop {
            let latest_block = block_stream.recv().await?;
            self.handle_latest_block(latest_block).await?;
        }
    }

    /// Sync the indexer to the latest block
    async fn sync_to_head(&mut self) -> IndexerResult {
        let latest_block = match self
            .provider
            .get_block_by_number(BlockNumberOrTag::Latest, BlockTransactionsKind::Hashes)
            .await?
        {
            Some(block) => block,
            None => {
                warn!("Latest block not found");
                return Ok(());
            }
        };

        let latest_block_header = latest_block.header();
        self.sync(UpdateBlock {
            block_number: latest_block_header.number(),
            block_hash: latest_block_header.hash(),
            parent_hash: latest_block_header.parent_hash(),
        })
        .await?;

        let finalized_block = match self
            .provider
            .get_block_by_number(BlockNumberOrTag::Finalized, BlockTransactionsKind::Hashes)
            .await?
        {
            Some(block) => block,
            None => {
                warn!("Finalized block not found");
                return Ok(());
            }
        };

        self.move_finalized_pending_modifications(finalized_block.header().number())
            .await?;

        self.is_synced = true;

        Ok(())
    }

    /// Prune the pending modifications and block hashes trees up to a block number
    fn prune_to(&self, to: u64) -> IndexerResult<Vec<PendingModification>> {
        let to = ser(&to)?;

        let mut pending_modifications = Vec::new();
        while let Some((key, _)) = self.block_hash_tree.pop_first_in_range(..to.clone())? {
            if let Some(pending_mods) = self.pending_modifications_tree.remove(&key)? {
                let pending_mods: Vec<PendingModification> = de(&pending_mods)?;
                pending_modifications.extend(pending_mods);
            }
        }

        Ok(pending_modifications)
    }

    /// Prune the pending modifications and block hashes trees from a block number
    fn prune_from(&self, from: u64) -> IndexerResult {
        let from = ser(&from)?;

        while let Some((key, _)) = self.block_hash_tree.pop_first_in_range(from.clone()..)? {
            self.pending_modifications_tree.remove(&key)?;
        }

        Ok(())
    }

    /// Finds the common ancestor
    /// Traverses from the cursor backwords until it finds a common ancestor in the block_hashes
    /// tree.
    #[instrument(skip(self))]
    async fn find_common_ancestor(&self, cursor_hash: B256) -> IndexerResult<u64> {
        let block_hashes_tree = &self.block_hash_tree;

        let mut cursor_hash = cursor_hash;
        loop {
            let cursor = self
                .provider
                .get_block_by_hash(cursor_hash, BlockTransactionsKind::Hashes)
                .await?
                .ok_or(IndexerError::ParentBlockNotFound)?;
            let cursor_header = cursor.header();

            cursor_hash = cursor_header.parent_hash();
            if let Some(hash) = block_hashes_tree.get(ser(&cursor_header.number())?)? {
                if hash == ser(&cursor_header.hash())? {
                    return Ok(cursor_header.number());
                }
            } else {
                return Err(IndexerError::NoCommonAncestor);
            }
        }
    }

    /// Handle the latest block, sync the indexer to the latest block, and moving finalized
    /// pending modifications to the store
    pub async fn handle_latest_block(&self, header: N::HeaderResponse) -> IndexerResult {
        self.sync(UpdateBlock {
            block_number: header.number(),
            block_hash: header.hash(),
            parent_hash: header.parent_hash(),
        })
        .await?;

        // get finalized and move finalized pending mods to store.
        if let Some(finalized_block) = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Finalized, BlockTransactionsKind::Hashes)
            .await?
        {
            self.move_finalized_pending_modifications(finalized_block.header().number())
                .await?;
        }

        Ok(())
    }

    /// Sync the indexer to the latest block
    /// If no block has been indexed, indexes from block 0.
    ///
    /// Otherwise checks for reorg.
    /// If reorg, prunes the pending modifications and block hashes trees.
    /// Then indexes the new blocks.
    /// If no block has been indexed, indexes from block 0.
    pub async fn sync(&self, update_block: UpdateBlock) -> IndexerResult {
        let last_indexed_block_num_hash = self.get_last_indexed_block_num_hash()?;

        let from;
        // If a block has been indexed, check if the new block is part of the same chain.
        if let Some(last_indexed_block_num_hash) = last_indexed_block_num_hash {
            let is_reorg =
                check_if_reorged(&self.provider, &update_block, last_indexed_block_num_hash)
                    .await?;

            if is_reorg {
                let common_ancestor = self.find_common_ancestor(update_block.parent_hash).await?;
                from = common_ancestor + 1;
                self.prune_from(from)?;
            } else {
                from = last_indexed_block_num_hash.number + 1;
            }
        } else {
            from = 0;
        };

        self.index_range(from, update_block.block_number).await?;
        Ok(())
    }

    /// Move finalized pending modifications to the store
    /// Prune the pending modifications and block hashes trees
    async fn move_finalized_pending_modifications(
        &self,
        finalized_block_number: u64,
    ) -> IndexerResult {
        // Delay pruning genesis block until the first finalized block is indexed.
        // Otherwise it would immediately prune the genesis block and would not be able to find a
        // common ancestor if there was a reorg to the genesis block.
        if finalized_block_number == 0 {
            return Ok(());
        }

        let pending_modifications = self.prune_to(finalized_block_number + 1)?;
        self.store
            .apply_pending_modifications(pending_modifications)?;

        Ok(())
    }

    /// Fetch the events from the State Oracle contract.
    /// Store the events in the pending_modifications tree for the indexed blocks.
    #[instrument(skip(self))]
    async fn index_range(&self, from: u64, to: u64) -> IndexerResult {
        let filter = Filter::new()
            .address(self.state_oracle)
            .from_block(BlockNumberOrTag::Number(from))
            .to_block(BlockNumberOrTag::Number(to));

        let logs = self.provider.get_logs(&filter).await?;

        // For ordered insertion of pending modifications
        let mut pending_modifications: BTreeMap<u64, BTreeMap<u64, PendingModification>> =
            BTreeMap::new();

        for log in logs {
            let log_index = log.log_index.ok_or(IndexerError::LogIndexMissing)?;
            let block_number = log.block_number.ok_or(IndexerError::BlockNumberMissing)?;

            if let Some(modification) = self
                .extract_pending_modifications(log.data(), log_index)
                .await?
            {
                pending_modifications
                    .entry(block_number)
                    .or_default()
                    .insert(log_index, modification);
            }
        }

        let mut pending_mods_batch = sled::Batch::default();

        for (block, log_map) in pending_modifications.iter() {
            let block_mods = log_map.values().collect::<Vec<&PendingModification>>();
            pending_mods_batch.insert(ser(block)?, ser(&block_mods)?);
        }

        self.pending_modifications_tree
            .apply_batch(pending_mods_batch)?;

        let mut block_hashes_batch = sled::Batch::default();
        for i in from..=to {
            let block_hash = self
                .provider
                .get_block(
                    BlockId::Number(BlockNumberOrTag::Number(i)),
                    BlockTransactionsKind::Hashes,
                )
                .await?
                .ok_or(IndexerError::BlockHashMissing)?
                .header()
                .hash();

            block_hashes_batch.insert(ser(&i)?, ser(&block_hash)?);
        }

        self.block_hash_tree.apply_batch(block_hashes_batch)?;

        Ok(())
    }

    /// Extract the pending modifications from the logs
    /// Resolves the bytecode via the assertion da.
    /// Extracts the fn selectors from the contract.
    async fn extract_pending_modifications(
        &self,
        log: &LogData,
        log_index: u64,
    ) -> IndexerResult<Option<PendingModification>> {
        let pending_mod_opt = match log.topics().first() {
            Some(&AssertionAdded::SIGNATURE_HASH) => {
                let topics = AssertionAdded::decode_topics(log.topics())?;
                let data = AssertionAdded::abi_decode_data(&log.data, true)?;
                let event = AssertionAdded::new(topics, data);

                let bytecode = self
                    .da_client
                    .fetch_assertion(event.assertionId)
                    .await?
                    .bytecode;

                let assertion_contract_res =
                    extract_assertion_contract(bytecode.clone(), &self.executor_config);

                match assertion_contract_res {
                    Ok((assertion_contract, trigger_recorder)) => {
                        let active_at_block = event
                            .activeAtBlock
                            .try_into()
                            .map_err(|_| IndexerError::BlockNumberExceedsU64)?;

                        Some(PendingModification::Add {
                            assertion_adopter: event.contractAddress,
                            assertion_contract,
                            trigger_recorder,
                            active_at_block,
                            log_index,
                        })
                    }
                    Err(e) => {
                        error!("Failed to extract assertion contract: {:?}", e);
                        None
                    }
                }
            }

            Some(&AssertionRemoved::SIGNATURE_HASH) => {
                let topics = AssertionRemoved::decode_topics(log.topics())?;
                let data = AssertionRemoved::abi_decode_data(&log.data, true)?;
                let event = AssertionRemoved::new(topics, data);

                let inactive_at_block = event
                    .activeAtBlock
                    .try_into()
                    .map_err(|_| IndexerError::BlockNumberExceedsU64)?;

                Some(PendingModification::Remove {
                    assertion_contract_id: event.assertionId,
                    assertion_adopter: event.contractAddress,
                    inactive_at_block,
                    log_index,
                })
            }
            _ => None,
        };
        Ok(pending_mod_opt)
    }

    /// Get the last indexed block number and hash
    fn get_last_indexed_block_num_hash(&self) -> IndexerResult<Option<BlockNumHash>> {
        let last_indexed_block = self.block_hash_tree.last()?;
        if let Some(last_indexed_block) = last_indexed_block {
            Ok(Some(BlockNumHash {
                number: de(&last_indexed_block.0)?,
                hash: de(&last_indexed_block.1)?,
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod test_indexer {
    use super::*;
    use crate::{
        inspectors::{
            CallTracer,
            TriggerRecorder,
        },
        primitives::{
            Address,
            AssertionContract,
            U256,
        },
        test_utils::{
            anvil_provider,
            bytecode,
            deployed_bytecode,
            mine_block,
            selector_assertion,
            FN_SELECTOR,
        },
    };

    use sled::Config;

    use tokio::task::JoinHandle;

    use tokio::net::TcpListener;

    use alloy_network::Ethereum;
    use alloy_network::{
        EthereumWallet,
        TransactionBuilder,
    };
    use alloy_node_bindings::AnvilInstance;
    use alloy_provider::ext::AnvilApi;
    use alloy_rpc_types::TransactionRequest;
    use alloy_rpc_types_anvil::MineOptions;
    use alloy_signer_local::PrivateKeySigner;
    use alloy_sol_types::{
        sol,
        SolCall,
    };

    use assertion_da_server::spawn_processes;

    sol! {

        function addAssertion(address contractAddress, bytes32 assertionId) public {}

        function removeAssertion(address contractAddress, bytes32 assertionId) public {}

    }

    async fn setup() -> (Indexer<Ethereum>, JoinHandle<()>, AnvilInstance) {
        let (provider, anvil) = anvil_provider().await;

        let state_oracle = Address::new([0; 20]);
        let db = Config::tmp().unwrap().open().unwrap();
        let (handle, port) = mock_da_server().await;
        let store = AssertionStore::new_ephemeral().unwrap();
        let da_client = DaClient::new(&format!("http://127.0.0.1:{port}")).unwrap();

        (
            Indexer::new(IndexerCfg {
                provider,
                block_hash_tree: db.open_tree("block_hashes").unwrap(),
                pending_modifications_tree: db.open_tree("pending_modifications").unwrap(),
                store,
                da_client,
                state_oracle,
                executor_config: ExecutorConfig::default(),
            }),
            handle,
            anvil,
        )
    }

    async fn send_modifications(indexer: &Indexer<Ethereum>) -> Address {
        let registration_mock = deployed_bytecode("AssertionRegistration.sol:RegistrationMock");
        let chain_id = indexer.provider.get_chain_id().await.unwrap();

        indexer
            .provider
            .anvil_set_code(indexer.state_oracle, registration_mock)
            .await
            .unwrap();
        let signer = PrivateKeySigner::random();
        let wallet = EthereumWallet::from(signer.clone());

        let protocol_addr = Address::random();

        let (assertion_contract, _) = selector_assertion();
        let assertion_id = assertion_contract.id;

        let assertion = bytecode(FN_SELECTOR);

        indexer
            .da_client
            .submit_assertion(assertion.clone())
            .await
            .unwrap();

        indexer
            .provider
            .anvil_set_balance(signer.address(), U256::MAX)
            .await
            .unwrap();

        for (nonce, input) in [
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
                .with_to(indexer.state_oracle)
                .with_input(input)
                .with_nonce(nonce)
                .with_chain_id(chain_id)
                .with_gas_limit(25_000)
                .with_max_priority_fee_per_gas(1_000_000_000)
                .with_max_fee_per_gas(20_000_000_000);

            let tx_envelope = tx.build(&wallet).await.unwrap();

            let _ = indexer
                .provider
                .send_tx_envelope(tx_envelope)
                .await
                .unwrap();
        }

        protocol_addr
    }

    async fn mock_da_server() -> (JoinHandle<()>, u16) {
        unsafe {
            std::env::set_var(
                "PRIVATE_KEY",
                "00973f42a0620b6fee12391c525daeb64097412e117b8f09a2742e06ca14e0ae",
            );
        }

        let config = assertion_da_server::Config {
            db_path: tempfile::tempdir()
                .unwrap()
                .path()
                .to_str()
                .unwrap()
                .to_string(),
            listen_addr: "127.0.0.1:0".to_owned(),
            cache_size: 1024,
        };

        println!("Binding to: {}", config.listen_addr);
        let listener = TcpListener::bind(&config.listen_addr).await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        println!("Bound to: {}", local_addr);

        // Try to open the sled db
        let db: sled::Db<1024> = sled::Config::new()
            .path(&config.db_path)
            .cache_capacity_bytes(config.cache_size)
            .open()
            .unwrap();

        // We now want to spawn all internal processes inside of a loop in this macro,
        // tokio selecting them so we can essentially restart the whole assertion
        // loader on demand in case anything fails.

        let handle = tokio::spawn(async move { spawn_processes!(listener, db) });

        (handle, local_addr.port())
    }

    async fn mine_block_write_hash(indexer: &Indexer<Ethereum>) -> alloy_rpc_types::Header {
        let header = mine_block(&indexer.provider).await;

        indexer
            .block_hash_tree
            .insert(
                bincode::serialize(&header.number).unwrap(),
                bincode::serialize(&header.hash).unwrap(),
            )
            .unwrap();

        header
    }

    #[tokio::test]
    async fn test_index_range() {
        let (indexer, _da, _anvil) = setup().await;

        let assertion_adopter = send_modifications(&indexer).await;

        let block = indexer
            .provider
            .get_block_by_number(0.into(), Default::default())
            .await
            .unwrap()
            .unwrap();
        let block_number = block.header.number;

        assert_eq!(block_number, 0);

        let timestamp = block.header.inner.timestamp;

        let _ = indexer
            .provider
            .evm_mine(Some(MineOptions::Timestamp(Some(timestamp + 5))))
            .await;

        indexer.index_range(0, 1).await.unwrap();

        let pending_mods_tree = indexer.pending_modifications_tree;

        assert_eq!(pending_mods_tree.len(), 1);

        let key = bincode::serialize(&1_u64).unwrap();
        let value = pending_mods_tree.get(&key).unwrap().unwrap();

        let pending_modifications: Vec<PendingModification> = bincode::deserialize(&value).unwrap();

        let (assertion_contract, trigger_recorder) = selector_assertion();

        assert_eq!(
            pending_modifications,
            vec![
                PendingModification::Add {
                    assertion_adopter,
                    assertion_contract: assertion_contract.clone(),
                    trigger_recorder,
                    active_at_block: 65,
                    log_index: 0,
                },
                PendingModification::Remove {
                    inactive_at_block: 65,
                    log_index: 1,
                    assertion_contract_id: assertion_contract.id,
                    assertion_adopter,
                },
            ]
        );
    }

    #[tokio::test]
    async fn test_find_common_ancestor_shallow_reorg() {
        let (indexer, _da, _anvil) = setup().await;

        // Mine and store initial blocks
        let _ = mine_block_write_hash(&indexer).await;
        let _ = mine_block_write_hash(&indexer).await;
        let _ = mine_block_write_hash(&indexer).await;

        let latest_block_num = indexer.provider.get_block_number().await.unwrap();
        let reorg_depth = 2;

        // Create reorg by resetting to block 1
        let reorg_options = alloy_rpc_types_anvil::ReorgOptions {
            depth: reorg_depth,
            tx_block_pairs: vec![],
        };

        indexer.provider.anvil_reorg(reorg_options).await.unwrap();

        // Mine new block on different fork
        let new_block = mine_block_write_hash(&indexer).await;

        // Find common ancestor
        let common_ancestor = indexer
            .find_common_ancestor(new_block.inner.parent_hash)
            .await
            .unwrap();

        // Common ancestor should be block 1
        assert_eq!(common_ancestor, latest_block_num - reorg_depth);
    }

    #[tokio::test]
    async fn test_find_common_ancestor_missing_blocks() {
        let (indexer, _da, _anvil) = setup().await;

        // Mine block but don't store it
        let _ = mine_block(&indexer.provider).await;

        // Create reorg
        let reorg_options = alloy_rpc_types_anvil::ReorgOptions {
            depth: 1,
            tx_block_pairs: vec![],
        };
        indexer.provider.anvil_reorg(reorg_options).await.unwrap();

        let new_block1 = mine_block(&indexer.provider).await;

        // Should fail to find common ancestor since no blocks are stored
        let result = indexer
            .find_common_ancestor(new_block1.inner.parent_hash)
            .await;

        assert!(matches!(result, Err(IndexerError::NoCommonAncestor)));
    }

    #[tokio::test]
    async fn test_stream_blocks_normal_case() {
        let (indexer, _da, _anvil) = setup().await;

        // Set up initial state
        let block0 = indexer
            .provider
            .get_block_by_number(0.into(), Default::default())
            .await
            .unwrap()
            .unwrap();

        // Store initial block in db
        indexer
            .block_hash_tree
            .insert(
                bincode::serialize(&0u64).unwrap(),
                bincode::serialize(&block0.header.hash).unwrap(),
            )
            .unwrap();

        // Create a block subscription
        let mut block_stream = indexer.provider.subscribe_blocks().await.unwrap();

        // Mine a new block
        let _ = indexer.provider.evm_mine(None).await;

        // Process the new block
        let latest_block = block_stream.recv().await.unwrap();
        indexer.handle_latest_block(latest_block).await.unwrap();

        // Verify block was properly indexed; Should have both blocks (0 and 1)
        assert_eq!(indexer.block_hash_tree.len(), 2);
    }

    #[tokio::test]
    async fn test_reorg() {
        for sync_before_mining in [false, true] {
            let (mut indexer, _da, _anvil) = setup().await;

            if sync_before_mining {
                indexer.sync_to_head().await.unwrap();
            }

            // Mine block 1
            let block1 = mine_block(&indexer.provider).await;

            if !sync_before_mining {
                indexer.sync_to_head().await.unwrap();
            }

            // Create a block subscription
            let mut block_stream = indexer.provider.subscribe_blocks().await.unwrap();

            // Simulate a reorg
            let reorg_options = alloy_rpc_types_anvil::ReorgOptions {
                depth: 1,
                tx_block_pairs: vec![],
            };
            indexer.provider.anvil_reorg(reorg_options).await.unwrap();

            // Process the newly mined block
            let latest_block = block_stream.recv().await.unwrap();
            indexer
                .handle_latest_block(latest_block)
                .await
                .expect(&format!("sync before mining: {sync_before_mining}\nerror"));

            // Verify state after reorg
            let last_indexed_block = indexer.get_last_indexed_block_num_hash().unwrap().unwrap();

            // Hash should be different fodf same block num
            assert_ne!(last_indexed_block.hash, block1.hash);
            assert_eq!(last_indexed_block.number, block1.number);

            // Should still have 2 blocks (0 and 1)
            assert_eq!(indexer.block_hash_tree.len(), 2, "Block hashes len");
        }
    }

    #[tokio::test]
    async fn test_move_finalized() {
        let (mut indexer, _da, _anvil) = setup().await;

        indexer.provider.evm_mine(None).await.unwrap();
        indexer.provider.evm_mine(None).await.unwrap();

        indexer.sync_to_head().await.unwrap();
        let aa = Address::random();
        // Add pending modification
        let modification = PendingModification::Add {
            assertion_adopter: aa,
            assertion_contract: AssertionContract::default(),
            trigger_recorder: TriggerRecorder::default(),
            active_at_block: 3,
            log_index: 0,
        };

        indexer
            .pending_modifications_tree
            .insert(
                bincode::serialize(&2u64).unwrap(),
                bincode::serialize(&vec![modification]).unwrap(),
            )
            .unwrap();

        assert_eq!(indexer.pending_modifications_tree.len(), 1);
        assert_eq!(indexer.block_hash_tree.len(), 3);

        let run_0 = (1, 3); // finalize block 0 -> Do nothing
        let run_1 = (1, 1); // finalize block 1 -> Remove block 0 and Block 1 hashes
        let run_2 = (0, 0); // finalize block 2 -> Remove Block 2 hashes and Pending modification

        for (finalized_block, (expected_pending_mods, expected_block_hashes)) in
            [run_0, run_1, run_2].iter().enumerate()
        {
            indexer
                .move_finalized_pending_modifications(finalized_block as u64)
                .await
                .unwrap();

            let pending_mods_tree = &indexer.pending_modifications_tree;
            let block_hashes_tree = &indexer.block_hash_tree;

            assert_eq!(
                pending_mods_tree.len(),
                *expected_pending_mods,
                "finalized block: {finalized_block}"
            );
            assert_eq!(
                block_hashes_tree.len(),
                *expected_block_hashes,
                "finalized block: {finalized_block}"
            );
        }
        let mut call_tracer = CallTracer::new();
        call_tracer.insert_trace(aa);

        let active_assertions_2 = indexer.store.read(&call_tracer, U256::from(2)).unwrap();
        let active_assertions_3 = indexer.store.read(&call_tracer, U256::from(3)).unwrap();

        assert_eq!(active_assertions_2.len(), 0);
        assert_eq!(active_assertions_3.len(), 1);
    }
}
