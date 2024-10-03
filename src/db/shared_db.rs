use crate::{
    db::{
        DatabaseCommit,
        NotFoundError,
        DatabaseRef,
        MemoryDb,
    },
    primitives::{
        Account,
        AccountInfo,
        Address,
        Bytecode,
        B256,
        U256,
    },
};

use std::{
    collections::HashMap,
    sync::{
        Arc,
        RwLock,
    },
};

#[derive(Debug, Clone, Default)]
pub struct SharedDB {
    db: Arc<RwLock<MemoryDb>>,
}

impl SharedDB {
    /// Create new `SharedDB` from a `MemoryDb`.
    // Note: depending on how large the MemoryDb this might nuke performance
    pub fn new(memory_db: MemoryDb) -> Self {
        Self {
            db: Arc::new(RwLock::new(memory_db)),
        }
    }
}

impl DatabaseRef for SharedDB {
    type Error = NotFoundError;
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .basic_ref(address)
    }
    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .block_hash_ref(number)
    }
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .storage_ref(address, index)
    }
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .code_by_hash_ref(code_hash)
    }
}

//Replace with commit_block in trait PhDB trait
impl DatabaseCommit for SharedDB {
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        self.db
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .commit(changes);
    }
}
