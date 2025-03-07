use crate::{
    db::{
        fork_db::ForkDb,
        fs::{
            FsDb,
            FsDbError,
        },
        DatabaseRef,
        MemoryDb,
        NotFoundError,
    },
    primitives::{
        AccountInfo,
        Address,
        BlockChanges,
        BlockEnv,
        Bytecode,
        TxEnv,
        UpdateBlock,
        B256,
        U256,
    },
    store::{
        AssertionStore,
        AssertionStoreError,
    },
    utils::{
        fill_tx_env,
        reorg_utils::{
            check_if_reorged,
            CheckIfReorgedError,
        },
    },
    AssertionExecutor,
    ExecutorConfig,
    ExecutorError,
};

use revm::primitives::BlobExcessGasAndPrice;

use std::{
    path::Path,
    sync::{
        Arc,
        Mutex,
        RwLock,
    },
};

use sled::Config;

use tracing::error;

use alloy_rpc_types::{
    BlockId,
    BlockNumHash,
    BlockTransactionsKind,
};

use alloy_provider::{
    Provider,
    RootProvider,
};

use alloy_transport::TransportError;

#[derive(Debug, thiserror::Error)]
pub enum SharedDbError {
    #[error("FsDbError: {0}")]
    FsDbError(#[from] FsDbError),
    #[error("TransportError: {0}")]
    TransportError(#[from] TransportError),
    #[error("Latest block not returned by RPC")]
    LatestBlockNotReturned,
    #[error("New canonical block not found")]
    NewCanonicalBlockNotFound,
    #[error("Check if reorged error")]
    CheckIfReorgedError(#[from] CheckIfReorgedError),
    #[error("Signature recovery failed")]
    SignatureRecoveryFailed(#[from] alloy::primitives::SignatureError),
    #[error("Assertion execution failed")]
    AssertionExecutionFailed(#[from] ExecutorError<NotFoundError>),
    #[error("Store initialization failed")]
    StoreInitializationFailed(AssertionStoreError),
}

/// A shared database that maintains a memory database and a file-system database.
///
/// The memory database is used for fast access to recent data, while the file-system
/// database is used for persistent storage.
/// BLOCKS_TO_RETAIN is the number of historical blocks to retain in memory.
#[derive(Debug, Clone)]
pub struct SharedDB<const BLOCKS_TO_RETAIN: usize = 64> {
    mem_db: Arc<RwLock<MemoryDb<BLOCKS_TO_RETAIN>>>,
    fs_db: Arc<Mutex<FsDb>>,
    provider: RootProvider,
    executor_config: ExecutorConfig,
}

impl<const BLOCKS_TO_RETAIN: usize> DatabaseRef for SharedDB<BLOCKS_TO_RETAIN> {
    type Error = NotFoundError;
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.mem_db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .basic_ref(address)
    }
    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.mem_db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .block_hash_ref(number)
    }
    fn storage_ref(&self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        self.mem_db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .storage_ref(address, slot)
    }
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.mem_db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .code_by_hash_ref(code_hash)
    }
}

impl<const BLOCKS_TO_RETAIN: usize> SharedDB<BLOCKS_TO_RETAIN> {
    /// Creates a new `SharedDb` struct from a path.
    pub fn new(
        path: &Path,
        provider: RootProvider,
        executor_config: ExecutorConfig,
    ) -> Result<Self, SharedDbError> {
        Ok(Self {
            provider,
            mem_db: Arc::new(RwLock::new(MemoryDb::default())),
            fs_db: Arc::new(Mutex::new(FsDb::new(path)?)),
            executor_config,
        })
    }

    /// Creates a new `SharedDb` struct from an existing `MemoryDb` and sled `Config`.
    pub fn new_with_config(
        mem_db: MemoryDb<BLOCKS_TO_RETAIN>,
        config: Config,
        provider: RootProvider,
        executor_config: ExecutorConfig,
    ) -> Result<Self, SharedDbError> {
        Ok(Self {
            provider,
            mem_db: Arc::new(RwLock::new(mem_db)),
            fs_db: Arc::new(Mutex::new(FsDb::new_with_config(config)?)),
            executor_config,
        })
    }

    /// Load the entire `FsDb` into the `MemoryDb`.
    pub async fn initialize(&mut self) -> Result<(), SharedDbError> {
        {
            let mut mem_lock = self.mem_db.write().unwrap_or_else(|e| e.into_inner());

            let fs_lock = self.fs_db.lock().unwrap_or_else(|e| e.into_inner());

            mem_lock.storage = fs_lock.load_storage()?;
            mem_lock.basic = fs_lock.load_basic()?;
            (
                mem_lock.block_hashes,
                mem_lock.canonical_block_num,
                mem_lock.canonical_block_hash,
            ) = fs_lock.load_block_hashes()?;
            mem_lock.code_by_hash = fs_lock.load_code_by_hash()?;
        }

        // get latest block hash
        let latest_block_hash = self
            .provider
            .get_block(BlockId::latest(), BlockTransactionsKind::Hashes)
            .await?
            .ok_or(SharedDbError::LatestBlockNotReturned)?
            .header
            .hash;

        self.canonicalize(latest_block_hash).await?;

        Ok(())
    }

    /// Canonicalizes the database, with an expected parent block hash.
    pub async fn canonicalize(&self, block_hash: B256) -> Result<(), SharedDbError> {
        let last_indexed_block = {
            let db = self.mem_db.write().unwrap_or_else(|e| e.into_inner());

            BlockNumHash {
                number: db.canonical_block_num,
                hash: db.canonical_block_hash,
            }
        };

        // Return early if the requested block hash is already the canonical block hash.
        if last_indexed_block.hash == block_hash {
            return Ok(());
        }

        let new_block = self
            .provider
            .get_block_by_hash(block_hash, BlockTransactionsKind::Hashes)
            .await?
            .ok_or(SharedDbError::NewCanonicalBlockNotFound)?;

        // Check if reorg occurred
        let is_reorged = check_if_reorged(
            &self.provider,
            &UpdateBlock {
                block_number: new_block.header.number,
                block_hash: new_block.header.hash,
                parent_hash: new_block.header.parent_hash,
            },
            last_indexed_block,
        )
        .await?;

        // If reorged, handle reorg by calling handle reorg on mem db, and passing update parameters to fs_db
        // if some are returned.
        if is_reorged {
            let mut db = self.mem_db.write().unwrap_or_else(|e| e.into_inner());
            if let Some(fs_db_params) = db.handle_reorg(block_hash) {
                let fs_db_lock = self.fs_db.lock().unwrap_or_else(|e| e.into_inner());
                fs_db_lock.handle_reorg(fs_db_params).map_err(|e| {
                    error!("Error handling reorg in FsDb: {:?}", e);
                    e
                })?;
            }
            // If handling the reorg canonicallizes the chain, return early.
            if db.canonical_block_hash == block_hash {
                return Ok(());
            }
        }
        let executor = AssertionExecutor {
            db: self.clone(),
            config: self.executor_config.clone(),
            store: AssertionStore::new_ephemeral()
                .map_err(SharedDbError::StoreInitializationFailed)?,
        };

        let mut cursor_hash = new_block.header.parent_hash;
        // Sync to the latest block
        for _ in new_block.header.number..last_indexed_block.number {
            let block = self
                .provider
                .get_block(cursor_hash.into(), BlockTransactionsKind::Full)
                .await?
                .ok_or(SharedDbError::NewCanonicalBlockNotFound)?;

            cursor_hash = block.header.parent_hash;

            let transactions = block.transactions.into_transactions(); //.as_transactions();

            let mut blob_excess_gas_and_price = None;

            if let Some(excess_blob_gas) = block.header.inner.excess_blob_gas {
                blob_excess_gas_and_price = Some(BlobExcessGasAndPrice::new(excess_blob_gas, true));
            }

            let block_env = BlockEnv {
                basefee: U256::from(block.header.inner.base_fee_per_gas.unwrap_or_default()),
                gas_limit: U256::from(block.header.gas_limit),
                prevrandao: Some(block.header.inner.mix_hash),
                difficulty: U256::from(block.header.difficulty),
                number: U256::from(block.header.number),
                coinbase: block.header.inner.beneficiary,
                timestamp: U256::from(block.header.timestamp),
                blob_excess_gas_and_price,
            };

            let mut block_changes = BlockChanges::new(block.header.number, block.header.hash);

            for tx in transactions {
                let mut tx_env = TxEnv::default();
                let signer = tx.inner.recover_signer()?;
                fill_tx_env(tx.inner, &mut tx_env, signer);

                let state_changes = executor
                    .execute_forked_tx(block_env.clone(), tx_env, &mut self.fork())?
                    .1
                    .state;

                block_changes.merge_state(state_changes);
            }

            let block_changes = self
                .mem_db
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .commit_block(block_changes);
            let fs_db_lock = self.fs_db.lock().unwrap_or_else(|e| e.into_inner());

            fs_db_lock.commit_block(block_changes).map_err(|e| {
                error!("Error committing block to FsDb: {:?}", e);
                e
            })?;
        }

        Ok(())
    }

    pub fn commit_block(&mut self, block_changes: BlockChanges) -> Result<(), SharedDbError> {
        let mut mem_db_lock = self.mem_db.write().unwrap_or_else(|e| e.into_inner());
        let fs_db_params = mem_db_lock.commit_block(block_changes);

        let fs_db_lock = self.fs_db.lock().unwrap_or_else(|e| e.into_inner());
        fs_db_lock.commit_block(fs_db_params).map_err(|e| {
            error!("Error committing block to FsDb: {:?}", e);
            e
        })?;

        Ok(())
    }

    /// Commits the entire memory database to the file-system database.
    pub fn commit_mem_db_to_fs(&self) -> Result<(), SharedDbError> {
        // Get fsdb lock
        let fs_db_lock = self.fs_db.lock().unwrap_or_else(|e| e.into_inner());

        // Get mem_db lock
        let mem_db_lock = self.mem_db.write().unwrap_or_else(|e| e.into_inner());

        fs_db_lock.commit_memory_db(&mem_db_lock)?;

        Ok(())
    }

    pub fn fork(&self) -> ForkDb<Self> {
        ForkDb::new(self.clone())
    }
}

//TODO: test block hash not found

#[cfg(any(test, feature = "test"))]
impl<const BLOCKS_TO_RETAIN: usize> SharedDB<BLOCKS_TO_RETAIN> {
    /// Creates a new `SharedDb` struct for testing. Spawns an Anvil instance
    pub async fn new_test() -> Self {
        use alloy_node_bindings::Anvil;
        use alloy_provider::{
            ProviderBuilder,
            WsConnect,
        };

        let anvil = Anvil::new().spawn();
        let provider = ProviderBuilder::new()
            .on_ws(WsConnect::new(anvil.ws_endpoint()))
            .await
            .unwrap();

        #[allow(deprecated)]
        let provider = provider.root().clone().boxed();

        Self {
            executor_config: ExecutorConfig::default(),
            provider,
            mem_db: Arc::new(RwLock::new(MemoryDb::default())),
            fs_db: Arc::new(Mutex::new(FsDb::new_test())),
        }
    }
}
