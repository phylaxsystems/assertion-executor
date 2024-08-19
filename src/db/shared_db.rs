use crate::{
    db::{
        memory_db::MemoryDB,
        DatabaseCommit,
        DatabaseRef,
        NotFoundError,
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

use std::sync::{
    Arc,
    RwLock,
};

/// A shared database that can be used by multiple threads.
/// Implemented using an Arc<RwLock<MemoryDB>>.
/// Read operations are done in parallel, done either at the end of every tx or the end of every
/// complete block to avoid contention.
#[derive(Debug, Clone, Default)]
pub struct SharedDB {
    pub db: Arc<RwLock<MemoryDB>>,
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

impl DatabaseCommit for SharedDB {
    fn commit(&mut self, changes: std::collections::HashMap<Address, Account>) {
        self.db
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .commit(changes)
    }
}
