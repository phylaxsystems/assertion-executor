use crate::{
    db::{
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

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct MemoryDB {
    basic: HashMap<Address, AccountInfo>,
    block_hashes: HashMap<u64, B256>,
    storage: HashMap<Address, HashMap<U256, U256>>,
    code_by_hash: HashMap<B256, Bytecode>,
}

impl DatabaseRef for MemoryDB {
    type Error = NotFoundError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.basic.get(&address).cloned())
    }
    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.block_hashes.get(&number).cloned().ok_or(NotFoundError)
    }
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Ok(self
            .storage
            .get(&address)
            .and_then(|s| s.get(&index))
            .cloned()
            .unwrap_or_default())
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash
            .get(&code_hash)
            .cloned()
            .ok_or(NotFoundError)
    }
}

impl DatabaseCommit for MemoryDB {
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        for (address, account) in changes {
            if account.is_selfdestructed() {
                self.basic.remove(&address);
                self.storage.remove(&address);
                continue;
            }

            if let Some(ref code) = account.info.code {
                self.code_by_hash
                    .insert(account.info.code_hash, code.clone());

                self.storage.entry(address).or_default().extend(
                    account
                        .storage
                        .into_iter()
                        .map(|(key, value)| (key, value.present_value())),
                );
            }
            self.basic.insert(address, account.info);
        }
    }
}
