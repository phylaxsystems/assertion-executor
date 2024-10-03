use crate::db::{
    DatabaseCommit,
    DatabaseRef,
    NotFoundError,
};
use crate::primitives::{
    Account,
    AccountInfo,
    Address,
    BlockChanges,
    Bytecode,
    ValueHistory,
    B256,
    U256,
};

use std::collections::{
    HashMap,
    VecDeque,
};

use reth_primitives_traits::{
    Account as RethAccount,
    Bytecode as RethBytecode,
    StorageEntry,
};

use reth_db::table::{
    Decompress,
    Table,
};

use revm::primitives::FixedBytes;
use sled::Db as SledDb;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum MemoryDbError {
    #[error("Failed to load MemoryDb from sled!")]
    LoadFail,
}

/// Contains current and past state of an EVM chain.
#[derive(Debug, Clone, Default)]
pub struct MemoryDb {
    /// Maps addresses to storage slots and their history indexed by block.
    pub(super) storage: HashMap<Address, HashMap<U256, ValueHistory<U256>>>,
    /// Maps addresses to their account info and indexes it by block.
    pub(super) basic: HashMap<Address, ValueHistory<AccountInfo>>,
    /// Maps block numbers to block hashes.
    pub(super) block_hashes: HashMap<u64, B256>,
    /// Maps bytecode hashes to bytecode.
    pub(super) code_by_hash: HashMap<B256, Bytecode>,

    /// Hash of the head block.
    pub(super) canonical_block_hash: B256,
    /// Block number of the head block.
    pub(super) canonical_block_num: u64,

    /// Block Changes for the blocks that have not been pruned.
    pub(super) block_changes: VecDeque<BlockChanges>,
}

impl MemoryDb {
    /// Iterates over all the KV pairs and loads them into the corresponding struct.
    fn load_tables_into_db<const LEAF_FANOUT: usize>(
        sled_db: &SledDb<LEAF_FANOUT>,
        mut memory_db: MemoryDb,
    ) -> Result<MemoryDb, MemoryDbError> {
        // Define which tables we're extracting
        //
        // *** !!! NOTE: THE ORDER OF THE TABELS MUST ABSOLUTELY NOT CHANGE !!! ***
        // *** !!! THINGS WILL NOT WORK AS INTENDED IF YOU CHANGE THIS !!! ***
        let reth_tables = &[
            "HeaderNumbers",
            "PlainAccountState",
            "Bytecodes",
            "PlainStorageState",
        ];

        // For populating the `canonical_block_hash` field
        let mut canonical_block_hash = Default::default();
        // For populating the `canonical_block_num` field
        let mut canonical_block_num = 0;
        // Because we do not have the full context of accounts in `PlainAccountState` we
        // need to temporarily store contract data in a map keyed by address.
        // TODO: potential future optimization to get rid of the `code_by_hash` field.
        let mut address_keyed_contracts: HashMap<Address, (FixedBytes<32>, Bytecode)> =
            Default::default();

        for table_name in reth_tables {
            match *table_name {
                // <Hash, Blocknumber>
                // Loads `block_hashes` as well as the `canonical_block_hash` and `canonical_block_num`
                // NOTE: this table must be deserialized first
                "HeaderNumbers" => {
                    let sled_table = sled_db.open_tree(table_name).unwrap();

                    sled_table.iter().for_each(|item| {
                        if let Ok((key, value)) = item {
                            let key: B256 = B256::new(key.as_ref().try_into().unwrap());

                            let value: &[u8] = value.as_ref();
                            let value: u64 = u64::from_le_bytes(value.try_into().unwrap());

                            // check if we have a higher block number
                            if canonical_block_num < value {
                                canonical_block_num = value;
                                canonical_block_hash = key;
                            }

                            memory_db.block_hashes.insert(value, key);
                        }
                    });

                    // Commit canonical block hash and number
                    memory_db.canonical_block_hash = canonical_block_hash;
                    memory_db.canonical_block_num = canonical_block_num;
                }
                // <Address, Bytecode>
                "Bytecodes" => {
                    let sled_table = sled_db.open_tree(table_name).unwrap();

                    sled_table.iter().for_each(|item| {
                        if let Ok((key, value)) = item {
                            let key: B256 = B256::new(key.as_ref().try_into().unwrap());
                            let address: Address = Address::new(key.as_slice().try_into().unwrap());

                            let value: Vec<u8> = value.as_ref().into();
                            let value: RethBytecode = RethBytecode::decompress(&value).unwrap();

                            // Now convert this to revm `Bytecode`
                            // absolutely insane typing
                            let bytecode: Bytecode =
                                Bytecode::new_raw_checked(value.0.bytecode().to_vec().into())
                                    .unwrap();
                            let hash = value.0.hash_slow().as_slice().try_into().unwrap();

                            // Insert into the temp address keyed contracts map
                            address_keyed_contracts.insert(address, (hash, bytecode.clone()));

                            // Insert into the memory_db
                            memory_db.code_by_hash.insert(hash, bytecode);
                        }
                    });
                }
                // <Address, Account>
                // Loads `basic`
                // NEEDS `Bytecodes` to be completed before this/
                "PlainAccountState" => {
                    let sled_table = sled_db.open_tree(table_name).unwrap();

                    sled_table.iter().for_each(|item| {
                        if let Ok((key, value)) = item {
                            let key: Address = Address::new(key.as_ref().try_into().unwrap());

                            let value: reth_primitives_traits::Account =
                                RethAccount::decompress(&value).unwrap();
                            let value: <reth_db::PlainAccountState as Table>::Value = value;

                            // We have to convert `PlainAccountState` to `AccountInfo`
                            // `AccountInfo` contains some additional data we need that is not

                            // Get the code and code hash.
                            // If not present code_hash is default and code is None
                            let (code_hash, code) = match address_keyed_contracts.get(&key) {
                                Some((hash, code)) => {
                                    // Clone so we can remove from temp map
                                    let hash = *hash;
                                    let code = code.clone();

                                    // Remove the entry from the temp map
                                    address_keyed_contracts.remove(&key);

                                    (hash, Some(code))
                                }
                                None => (B256::ZERO, None),
                            };

                            let acount_info: AccountInfo = AccountInfo {
                                nonce: value.nonce,
                                balance: value.balance,
                                code_hash,
                                code,
                            };

                            let mut history = ValueHistory::new();
                            history.insert(canonical_block_num, acount_info);
                            memory_db.basic.insert(key, history);
                        }
                    });
                }
                // <Address, StorageEntry, B256>
                // Loads `storage`
                // NEEDS `HeaderNumbers` to be completed before this.
                // TODO: double check this?
                "PlainStorageState" => {
                    let sled_table = sled_db.open_tree(table_name).unwrap();

                    sled_table.iter().for_each(|item| {
                        if let Ok((key, value)) = item {
                            // Get the address
                            let key: Address = Address::new(key.as_ref().try_into().unwrap());

                            // Get a a struct containing <storage slot key, value>
                            let value: StorageEntry = StorageEntry::decompress(&value).unwrap();
                            // We can always create a new value history safely because values
                            // should only appear once.
                            let history: ValueHistory<U256> = ValueHistory::new();

                            // Multiple of the same key can however exist
                            // We want to either create a new hashmap or add an entry to an existing one
                            let storage = memory_db.storage.entry(key).or_default();
                            storage
                                .entry(value.key.into())
                                .or_default()
                                .insert(canonical_block_num, value.value);
                        }
                    });
                }
                _ => {
                    return Err(MemoryDbError::LoadFail);
                }
            }
        }

        Ok(memory_db)
    }

    /// Opens a `sled` Db that has had exported and transformed data from `reth` MDBX with
    /// `mending::transform_reth_tables`.
    pub fn new_from_exported_sled<const LEAF_FANOUT: usize>(
        sled_db: &SledDb<LEAF_FANOUT>,
    ) -> Result<Self, MemoryDbError> {
        let memory_db = MemoryDb::default();

        Self::load_tables_into_db::<LEAF_FANOUT>(sled_db, memory_db)
    }
}

impl DatabaseRef for MemoryDb {
    type Error = NotFoundError;
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self
            .basic
            .get(&address)
            .and_then(|v| v.get_latest())
            .cloned())
    }
    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.block_hashes.get(&number).cloned().ok_or(NotFoundError)
    }
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Ok(*self
            .storage
            .get(&address)
            .and_then(|s| s.get(&index).map(|v| v.get_latest().unwrap_or(&U256::ZERO)))
            .unwrap_or(&U256::ZERO))
    }
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash
            .get(&code_hash)
            .cloned()
            .ok_or(NotFoundError)
    }
}

//Replace with commit_block in trait PhDB trait
//Will need to prune history and update sled
impl DatabaseCommit for MemoryDb {
    fn commit(&mut self, changes: std::collections::HashMap<Address, Account>) {
        self.canonical_block_num += 1;

        for (address, account) in &changes {
            if account.is_selfdestructed() {
                self.basic.remove(address);
                self.storage.remove(address);
                continue;
            }

            if let Some(ref code) = account.info.code {
                self.code_by_hash
                    .insert(account.info.code_hash, code.clone());

                let contract_storage = self.storage.entry(*address).or_default();
                for (index, slot) in account.storage.clone() {
                    contract_storage
                        .entry(index)
                        .or_default()
                        .insert(self.canonical_block_num, slot.present_value());
                }
            }
            self.basic
                .entry(*address)
                .or_default()
                .insert(self.canonical_block_num, account.info.clone());
        }
        self.block_changes.push_back(BlockChanges {
            block_num: self.canonical_block_num,
            block_hash: self.canonical_block_hash,
            state_changes: changes,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{
        Account,
        AccountInfo,
        B256,
        U256,
    };
    use crate::test_utils::random_bytes;
    use revm::primitives::{
        AccountStatus,
        Bytes,
        EvmStorageSlot,
    };
    use std::collections::HashMap;

    #[test]
    fn test_basic_ref() {
        let mut db = MemoryDb::default();
        let address = random_bytes().into();
        let code = Bytecode::LegacyRaw(Bytes::from(random_bytes::<64>()));
        let account_info = AccountInfo {
            nonce: 1,
            balance: U256::from(1000),
            code_hash: random_bytes(),
            code: Some(code.clone()),
        };

        db.commit(HashMap::from([(
            address,
            Account {
                info: account_info.clone(),
                storage: HashMap::new(),
                status: AccountStatus::Touched,
            },
        )]));

        let result = db.basic_ref(address);
        assert_eq!(result.unwrap(), Some(account_info));
    }

    #[test]
    fn test_block_hash_ref() {
        let mut db = MemoryDb::default();
        let block_number = 999;
        let block_hash = random_bytes();

        db.block_hashes.insert(block_number, block_hash);
        db.canonical_block_num = 1000;

        let result = db.block_hash_ref(block_number);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), block_hash);
    }

    #[test]
    fn test_storage_ref() {
        let mut db = MemoryDb::default();
        let address = random_bytes().into();
        let index = U256::from(1);
        let value = U256::from(100);

        // Commit the storage data
        db.commit(HashMap::from([(
            address,
            Account {
                info: AccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: B256::ZERO,
                    code: Some(Bytecode::LegacyRaw(Bytes::from(random_bytes::<64>()))),
                },
                storage: HashMap::from([(
                    index,
                    EvmStorageSlot {
                        present_value: value,
                        original_value: U256::ZERO,
                        is_cold: false,
                    },
                )]),
                status: AccountStatus::Touched,
            },
        )]));

        let result = db.storage_ref(address, index);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), value);

        let non_existent_index = U256::from(2);
        let result = db.storage_ref(address, non_existent_index);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), U256::ZERO);
    }

    //Should error if the code hash is not found
    #[test]
    fn test_code_by_hash_ref() {
        let mut db = MemoryDb::default();
        let code_hash = random_bytes();
        let code = Bytecode::LegacyRaw(Bytes::from(random_bytes::<96>()));

        // Commit the code data
        db.commit(HashMap::from([(
            random_bytes().into(),
            Account {
                info: AccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code: Some(code.clone()),
                    code_hash,
                },
                storage: HashMap::new(),
                status: AccountStatus::Touched,
            },
        )]));

        let result = db.code_by_hash_ref(code_hash);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), code);

        let non_existent_hash = random_bytes();
        let result = db.code_by_hash_ref(non_existent_hash);
        assert!(result.is_err());
    }

    #[test]
    fn test_commit() {
        let mut db = MemoryDb::default();
        let address = random_bytes().into();
        let new_account_info = AccountInfo {
            nonce: 1,
            balance: U256::from(2000),
            code_hash: random_bytes(),
            code: None,
        };
        let new_code = Bytecode::LegacyRaw(Bytes::from(random_bytes::<64>()));
        let new_storage = HashMap::from([(
            U256::from(1),
            EvmStorageSlot {
                present_value: U256::from(200),
                original_value: U256::ZERO,
                is_cold: false,
            },
        )]);

        let changes = HashMap::from([(
            address,
            Account {
                info: AccountInfo {
                    code: Some(new_code.clone()),
                    ..new_account_info.clone()
                },
                storage: new_storage.clone(),
                status: AccountStatus::Touched,
            },
        )]);

        db.commit(changes.clone());

        // Verify the changes
        assert_eq!(db.canonical_block_num, 1);

        assert_eq!(
            db.code_by_hash_ref(new_account_info.code_hash).unwrap(),
            new_code
        );

        assert_eq!(
            db.basic_ref(address).unwrap(),
            Some(new_account_info.clone())
        );
        assert_eq!(
            db.storage_ref(address, U256::from(1)).unwrap(),
            U256::from(200),
        );

        let block_changes = db.block_changes.pop_back().unwrap();

        assert_eq!(block_changes.block_num, 1);
        assert_eq!(block_changes.state_changes, changes);
    }
}
//TODO: Add integration test to verify revm handles block_hash access based on evm version
