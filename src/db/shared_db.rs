use crate::{
    db::{
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
        Bytecode,
        B256,
        U256,
    },
};

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

/// A shared database that maintains a memory database and a file-system database.
///
/// The memory database is used for fast access to recent data, while the file-system
/// database is used for persistent storage.
/// BLOCKS_TO_RETAIN is the number of historical blocks to retain in memory.
#[derive(Debug, Clone)]
pub struct SharedDB<const BLOCKS_TO_RETAIN: usize> {
    mem_db: Arc<RwLock<MemoryDb<BLOCKS_TO_RETAIN>>>,
    fs_db: Arc<Mutex<FsDb>>,
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
    pub fn new(path: &Path) -> Result<Self, FsDbError> {
        Ok(Self {
            mem_db: Arc::new(RwLock::new(MemoryDb::default())),
            fs_db: Arc::new(Mutex::new(FsDb::new(path)?)),
        })
    }

    /// Creates a new `SharedDb` struct from an existing `MemoryDb` and sled `Config`.
    pub fn new_with_config(
        mem_db: MemoryDb<BLOCKS_TO_RETAIN>,
        config: Config,
    ) -> Result<Self, FsDbError> {
        Ok(Self {
            mem_db: Arc::new(RwLock::new(mem_db)),
            fs_db: Arc::new(Mutex::new(FsDb::new_with_config(config)?)),
        })
    }

    pub fn canonicalize(&self, block_hash: B256) {
        let mut db = self.mem_db.write().unwrap_or_else(|e| e.into_inner());

        // Return early if the requested block hash is already the canonical block hash.
        if db.canonical_block_hash == block_hash {
            return;
        }

        let fs_db = self.fs_db.clone();
        if let Some(fs_db_params) = db.handle_reorg(block_hash) {
            let fs_db_lock = fs_db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = fs_db_lock.handle_reorg(fs_db_params).map_err(|e| {
                error!("Error handling reorg in FsDb: {:?}", e);
            });
        }
    }

    pub fn commit_block(&mut self, block_changes: BlockChanges) {
        let mut db = self.mem_db.write().unwrap_or_else(|e| e.into_inner());
        let fs_db_params = db.commit_block(block_changes);

        let fs_db = self.fs_db.clone();
        let fs_db_lock = fs_db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = fs_db_lock.commit_block(fs_db_params).map_err(|e| {
            error!("Error committing block to FsDb: {:?}", e);
        });
    }

    /// Commits the entire memory database to the file-system database.
    pub fn commit_mem_db_to_fs(&mut self) -> Result<(), FsDbError> {
        // Get fsdb lock
        let fs_db = self.fs_db.clone();
        let fs_db_lock = fs_db.lock().unwrap_or_else(|e| e.into_inner());

        // Get mem_db lock
        let mem_db = self.mem_db.write().unwrap_or_else(|e| e.into_inner());

        fs_db_lock.commit_memory_db(&mem_db)?;

        Ok(())
    }
}

//TODO: test block hash not found

#[cfg(any(test, feature = "test"))]
impl<const BLOCKS_TO_RETAIN: usize> SharedDB<BLOCKS_TO_RETAIN> {
    pub fn new_test() -> Self {
        Self {
            mem_db: Arc::new(RwLock::new(MemoryDb::default())),
            fs_db: Arc::new(Mutex::new(FsDb::new_test())),
        }
    }
}
