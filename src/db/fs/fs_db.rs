use crate::{
    db::{
        memory_db::EvmStorageHistory,
        MemoryDb,
    },
    primitives::{
        AccountInfo,
        Address,
        BlockChanges,
        Bytecode,
        ValueHistory,
        B256,
        U256,
    },
};

use super::{
    serde::{
        Deserialize,
        Serialize,
        StorageSlotKey,
    },
    FsDbError,
};

use std::{
    collections::{
        BTreeMap,
        HashMap,
    },
    path::Path,
};

use revm::primitives::FixedBytes;
use sled::{
    Batch,
    Config,
    Db,
};

/// A struct that holds the changes that need to be committed to the database for
/// a block.
#[derive(Debug, Default)]
pub struct FsCommitBlockParams {
    /// The changes to the state trie.
    pub(in crate::db) block_changes: BlockChanges,
    /// The changes to prune.
    pub(in crate::db) block_changes_to_remove: Vec<u64>,
    /// The code hashes to insert.
    pub(in crate::db) code_hashes_to_insert: Vec<(B256, Bytecode)>,
    /// The account value histories to insert.
    pub(in crate::db) account_value_histories: Vec<AccountValueHistory>,
    /// The storage value histories to insert.
    pub(in crate::db) storage_value_histories: Vec<StorageValueHistory>,
}

/// A struct that holds the changes that need to be made to the database to handle a reorg.
#[derive(Debug, Default)]
pub struct FsHandleReorgParams {
    pub(in crate::db) block_numbers_to_remove: Vec<u64>,
    pub(in crate::db) account_value_histories: Vec<AccountValueHistory>,
    pub(in crate::db) storage_value_histories: Vec<StorageValueHistory>,
}

/// A struct that holds the address key and the value history of an account.
#[derive(Debug, Default)]
pub struct AccountValueHistory {
    pub(in crate::db) address: Address,
    pub(in crate::db) value_history: ValueHistory<AccountInfo>,
}

/// A struct that holds the storage slot key and the value history of a storage slot.
#[derive(Debug, Default)]
pub struct StorageValueHistory {
    pub(in crate::db) key: StorageSlotKey,
    pub(in crate::db) value_history: ValueHistory<U256>,
}

/// A struct that holds the file-system database.
/// The database is a key-value store that stores the state of the blockchain.
#[derive(Debug, Clone)]
pub(in crate::db) struct FsDb {
    db: Db,
}

#[cfg(any(test, feature = "test"))]
impl FsDb {
    pub fn new_test() -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        Self::new(temp_dir.path()).unwrap()
    }
}

impl FsDb {
    const STORAGE: &'static str = "storage";
    const BASIC: &'static str = "basic";
    const BLOCK_HASHES: &'static str = "block_hashes";
    const CODE_BY_HASH: &'static str = "code_by_hash";
    const BLOCK_CHANGES: &'static str = "block_changes";

    /// Create a new file-system database.
    pub fn new(path: &Path) -> Result<Self, FsDbError> {
        Ok(Self {
            db: Config::new().path(path).open()?,
        })
    }

    /// Create a new file-system database from a sled config.
    pub fn new_with_config(config: Config) -> Result<Self, FsDbError> {
        Ok(Self { db: config.open()? })
    }

    /// Handle a reorg by removing the blocks from the database and inserting the new canonical
    /// block.
    pub fn handle_reorg(
        &self,
        FsHandleReorgParams {
            block_numbers_to_remove,
            account_value_histories,
            storage_value_histories,
        }: FsHandleReorgParams,
    ) -> Result<(), FsDbError> {
        let block_change_batch =
            block_numbers_to_remove
                .iter()
                .fold(Batch::default(), |mut batch, block_num| {
                    batch.remove(block_num.serialize());
                    batch
                });

        let block_hash_batch =
            block_numbers_to_remove
                .iter()
                .fold(Batch::default(), |mut batch, block_num| {
                    batch.remove(block_num.serialize());
                    batch
                });

        let basic_batch = Self::account_value_history_update_batch(account_value_histories);
        let storage_batch = Self::storage_value_history_update_batch(storage_value_histories);

        self.block_changes_tree()?.apply_batch(block_change_batch)?;
        self.block_hash_tree()?.apply_batch(block_hash_batch)?;
        self.basic_tree()?.apply_batch(basic_batch)?;
        self.storage_tree()?.apply_batch(storage_batch)?;

        Ok(())
    }

    /// Commit a block to the database.
    pub fn commit_block(
        &self,
        FsCommitBlockParams {
            block_changes,
            block_changes_to_remove,
            code_hashes_to_insert,
            account_value_histories,
            storage_value_histories,
        }: FsCommitBlockParams,
    ) -> Result<(), FsDbError> {
        let mut block_change_batch = Batch::default();
        block_change_batch.insert(
            block_changes.block_num.serialize(),
            block_changes.serialize(),
        );

        block_changes_to_remove.iter().for_each(|block_num| {
            block_change_batch.remove(block_num.serialize());
        });

        let code_by_hash_batch =
            code_hashes_to_insert
                .iter()
                .fold(Batch::default(), |mut batch, code_hash| {
                    batch.insert(code_hash.0.to_vec().serialize(), code_hash.1.bytes_slice());
                    batch
                });

        let basic_batch = Self::account_value_history_update_batch(account_value_histories);
        let storage_batch = Self::storage_value_history_update_batch(storage_value_histories);

        self.block_hash_tree()?.insert(
            block_changes.block_num.serialize(),
            block_changes.block_hash.serialize(),
        )?;
        self.block_changes_tree()?.apply_batch(block_change_batch)?;
        self.basic_tree()?.apply_batch(basic_batch)?;
        self.storage_tree()?.apply_batch(storage_batch)?;
        self.code_by_hash_tree()?.apply_batch(code_by_hash_batch)?;

        Ok(())
    }

    /// Commits an entire `MemoryDb` to the `FsDb`.
    pub fn commit_memory_db<const BLOCKS_TO_RETAIN: usize>(
        &self,
        mem_db: &MemoryDb<BLOCKS_TO_RETAIN>,
    ) -> Result<bool, FsDbError> {
        self.write_storage_to_tree(mem_db)?;
        self.write_basic_to_tree(mem_db)?;
        self.write_block_hashes_to_tree(mem_db)?;
        self.write_code_by_hash_to_tree(mem_db)?;

        Ok(true)
    }

    fn account_value_history_update_batch(value_historys: Vec<AccountValueHistory>) -> Batch {
        value_historys.into_iter().fold(
            Batch::default(),
            |mut batch,
             AccountValueHistory {
                 address,
                 value_history,
             }| {
                if value_history.is_empty() {
                    batch.remove(address.serialize());
                } else {
                    batch.insert(address.serialize(), value_history.serialize());
                }
                batch
            },
        )
    }

    fn storage_value_history_update_batch(value_historys: Vec<StorageValueHistory>) -> Batch {
        value_historys.into_iter().fold(
            Batch::default(),
            |mut batch, StorageValueHistory { key, value_history }| {
                if value_history.is_empty()
                    || (value_history.value_history.len() == 1
                        && value_history.get_latest().is_some_and(|val| val.is_zero()))
                {
                    batch.remove(key.serialize());
                } else {
                    batch.insert(key.serialize(), value_history.serialize());
                };
                batch
            },
        )
    }

    fn block_changes_tree(&self) -> Result<sled::Tree, FsDbError> {
        Ok(self.db.open_tree(Self::BLOCK_CHANGES)?)
    }
    fn block_hash_tree(&self) -> Result<sled::Tree, FsDbError> {
        Ok(self.db.open_tree(Self::BLOCK_HASHES)?)
    }
    fn basic_tree(&self) -> Result<sled::Tree, FsDbError> {
        Ok(self.db.open_tree(Self::BASIC)?)
    }
    fn code_by_hash_tree(&self) -> Result<sled::Tree, FsDbError> {
        Ok(self.db.open_tree(Self::CODE_BY_HASH)?)
    }
    fn storage_tree(&self) -> Result<sled::Tree, FsDbError> {
        Ok(self.db.open_tree(Self::STORAGE)?)
    }

    /// Write the entirety of the storage contents of a `MemoryDb` to a sled batch.
    fn write_storage_to_tree<const BLOCKS_TO_RETAIN: usize>(
        &self,
        mem_db: &MemoryDb<BLOCKS_TO_RETAIN>,
    ) -> Result<(), FsDbError> {
        let batch =
            mem_db
                .storage
                .iter()
                .fold(Batch::default(), |mut batch, (address, storage_slots)| {
                    // For each address and its storage slots
                    for (slot, value_history) in storage_slots {
                        // Create StorageSlotKey combining address and slot
                        let key = StorageSlotKey {
                            address: *address,
                            slot: *slot,
                        };

                        // Apply same rules as storage_value_history_update_batch
                        if value_history.is_empty()
                            || (value_history.value_history.len() == 1
                                && value_history.get_latest().is_some_and(|val| val.is_zero()))
                        {
                            batch.remove(key.serialize());
                        } else {
                            batch.insert(key.serialize(), value_history.serialize());
                        }
                    }
                    batch
                });

        self.storage_tree()?.apply_batch(batch)?;
        Ok(())
    }

    /// Write the entirety of the `basic` contents of a `MemoryDb` to a sled batch.
    fn write_basic_to_tree<const BLOCKS_TO_RETAIN: usize>(
        &self,
        mem_db: &MemoryDb<BLOCKS_TO_RETAIN>,
    ) -> Result<(), FsDbError> {
        let batch = mem_db
            .basic
            .iter()
            .fold(Batch::default(), |mut batch, (key, value)| {
                batch.insert(key.serialize(), value.serialize());
                batch
            });

        self.basic_tree()?.apply_batch(batch)?;
        Ok(())
    }

    /// Write the entirety of the `block_hashes` contents of a `MemoryDb` to a sled batch.
    fn write_block_hashes_to_tree<const BLOCKS_TO_RETAIN: usize>(
        &self,
        mem_db: &MemoryDb<BLOCKS_TO_RETAIN>,
    ) -> Result<(), FsDbError> {
        let batch = mem_db
            .block_hashes
            .iter()
            .fold(Batch::default(), |mut batch, (key, value)| {
                batch.insert(key.serialize(), value.serialize());
                batch
            });

        self.block_hash_tree()?.apply_batch(batch)?;
        Ok(())
    }

    /// Write the entirety of the `code_by_hash` contents of a `MemoryDb` to a sled batch.
    fn write_code_by_hash_to_tree<const BLOCKS_TO_RETAIN: usize>(
        &self,
        mem_db: &MemoryDb<BLOCKS_TO_RETAIN>,
    ) -> Result<(), FsDbError> {
        let batch = mem_db
            .code_by_hash
            .iter()
            .fold(Batch::default(), |mut batch, (key, value)| {
                batch.insert(key.serialize(), value.serialize());
                batch
            });

        self.code_by_hash_tree()?.apply_batch(batch)?;
        Ok(())
    }

    /// Loads the entirity of the `storage` table into an `EvmStorageHistory` struct.
    pub fn load_storage(&self) -> Result<EvmStorageHistory, FsDbError> {
        let tree = self.storage_tree()?;
        let mut storage = EvmStorageHistory::new();

        // Iterate over storage slots in the tree
        for result in tree.iter() {
            let (key, value) = result?;

            // StorageSlotKey (address + slot)
            let storage_key = StorageSlotKey::deserialize(key).unwrap();

            // ValueHistory<U256> for this specific slot
            let value_history = ValueHistory::<U256>::deserialize(value).unwrap();

            // Get or create the address's storage map
            let address_storage = storage.entry(storage_key.address).or_default();

            // Insert this slot's value history
            address_storage.insert(storage_key.slot, value_history);
        }

        Ok(storage)
    }

    /// Loads the entire `basic` table into a `HashMap<Address, ValueHistory<AccountInfo>>`.
    pub fn load_basic(&self) -> Result<HashMap<Address, ValueHistory<AccountInfo>>, FsDbError> {
        let tree = self.basic_tree()?;
        let mut basic = HashMap::new();

        for result in tree.iter() {
            let (key, value) = result?;

            // Get the address from the key bytes using from_slice
            let address = Address::from_slice(&key);

            // Deserialize value using our custom Deserialize trait
            let value_history = ValueHistory::<AccountInfo>::deserialize(value).unwrap();

            basic.insert(address, value_history);
        }

        Ok(basic)
    }

    /// Loads the entire `block_hashes` table into a `BTreeMap<u64, B256>`.
    pub fn load_block_hashes(
        &self,
    ) -> Result<(BTreeMap<u64, B256>, u64, FixedBytes<32>), FsDbError> {
        let tree = self.block_hash_tree()?;
        let mut block_hashes = BTreeMap::new();
        let mut canonical_block_num = 0;
        let mut canonical_block_hash = FixedBytes::<32>::default();

        for result in tree.iter() {
            let (key, value) = result?;

            // Both key and value use our custom Deserialize trait
            let block_num = u64::deserialize(key).unwrap();
            let block_hash = B256::deserialize(value).unwrap();

            if block_num > canonical_block_num {
                canonical_block_num = block_num;
                canonical_block_hash = block_hash;
            }

            block_hashes.insert(block_num, block_hash);
        }

        Ok((block_hashes, canonical_block_num, canonical_block_hash))
    }

    /// Loads the entire `code_by_hash` table into a `HashMap<B256, Bytecode>`.
    pub fn load_code_by_hash(&self) -> Result<HashMap<B256, Bytecode>, FsDbError> {
        let tree = self.code_by_hash_tree()?;
        let mut code_by_hash = HashMap::new();

        for result in tree.iter() {
            let (key, value) = result?;

            // Key uses B256::deserialize, value is raw bytes for Bytecode
            let code_hash = B256::deserialize(key).unwrap();
            let code = Bytecode::deserialize(value).unwrap();

            code_by_hash.insert(code_hash, code);
        }

        Ok(code_by_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::serde::{
            Deserialize,
            Serialize,
        },
        *,
    };

    use crate::{
        primitives::Bytes,
        test_utils::random_bytes,
    };

    #[cfg(test)]
    mod test_handle_reorg {
        use super::*;
        #[test]
        fn test_block_numbers_to_remove() {
            let db = FsDb::new_test();

            for block_num in 1..=3 {
                let block_changes = BlockChanges {
                    block_num,
                    block_hash: B256::from([0; 32]),
                    state_changes: Default::default(),
                };

                db.commit_block(FsCommitBlockParams {
                    block_changes,
                    ..Default::default()
                })
                .unwrap();
            }

            let block_changes_tree = db.block_changes_tree().unwrap();
            let block_hash_tree = db.block_hash_tree().unwrap();

            let block_numbers_to_remove = vec![2, 3];

            db.handle_reorg(FsHandleReorgParams {
                block_numbers_to_remove: block_numbers_to_remove.clone(),
                ..Default::default()
            })
            .unwrap();

            for block_num in block_numbers_to_remove {
                assert!(block_changes_tree
                    .get(block_num.serialize())
                    .unwrap()
                    .is_none());
                assert!(block_hash_tree
                    .get(block_num.serialize())
                    .unwrap()
                    .is_none());
            }
            // Assert the first block is still in the database
            assert!(block_changes_tree.get(1.serialize()).unwrap().is_some());
            assert!(block_hash_tree.get(1.serialize()).unwrap().is_some());
            assert_eq!(block_changes_tree.len(), 1);
            assert_eq!(block_hash_tree.len(), 1);
        }
        // Test account value histories
        #[test]
        fn test_account_value_histories() {
            let db = FsDb::new_test();

            let address = Address::from([1; 20]);

            let account_info_0 = AccountInfo {
                code: None,
                nonce: 1,
                balance: random_bytes().into(),
                code_hash: random_bytes(),
            };
            let account_info_1 = AccountInfo {
                code: Some(Bytecode::new_raw(Bytes::from(random_bytes::<22_000>()))),
                nonce: 1,
                balance: random_bytes().into(),
                code_hash: random_bytes(),
            };

            let mut value_history = ValueHistory::default();
            value_history.insert(1, account_info_0.clone());
            let value_history_reorg = value_history.clone();
            value_history.insert(2, account_info_1.clone());

            db.commit_block(FsCommitBlockParams {
                account_value_histories: vec![AccountValueHistory {
                    address,
                    value_history: value_history.clone(),
                }],

                ..Default::default()
            })
            .unwrap();

            db.handle_reorg(FsHandleReorgParams {
                account_value_histories: vec![AccountValueHistory {
                    address,
                    value_history: value_history_reorg.clone(),
                }],
                ..Default::default()
            })
            .unwrap();

            let basic_tree = db.basic_tree().unwrap();
            let stored_history = basic_tree.get(address.serialize()).unwrap().unwrap();

            assert_eq!(
                value_history_reorg,
                ValueHistory::<AccountInfo>::deserialize(stored_history).unwrap()
            );
        }
        //Test storage value histories
        #[test]
        fn test_storage_val_history() {
            let db = FsDb::new_test();

            // Create test data
            let storage_slot_key = StorageSlotKey {
                address: random_bytes().into(),
                slot: random_bytes().into(),
            };

            let mut value_history = ValueHistory::default();
            value_history.insert(1, random_bytes().into());
            let value_history_reorg = value_history.clone();
            value_history.insert(2, random_bytes().into());

            // Commit the storage value history
            db.commit_block(FsCommitBlockParams {
                storage_value_histories: vec![StorageValueHistory {
                    key: storage_slot_key.clone(),
                    value_history: value_history.clone(),
                }],
                ..Default::default()
            })
            .unwrap();

            // Handle the reorg
            db.handle_reorg(FsHandleReorgParams {
                storage_value_histories: vec![StorageValueHistory {
                    key: storage_slot_key.clone(),
                    value_history: value_history_reorg.clone(),
                }],
                ..Default::default()
            })
            .unwrap();

            let storage_tree = db.storage_tree().unwrap();
            // Verify the storage value history was stored correctly
            let stored_history = storage_tree
                .get(storage_slot_key.serialize())
                .unwrap()
                .unwrap();

            assert_eq!(
                value_history_reorg,
                ValueHistory::<U256>::deserialize(stored_history).unwrap()
            );
        }
    }

    #[cfg(test)]
    mod test_commit_block {
        use super::*;

        #[test]
        fn test_block_changes() {
            let db = FsDb::new_test();

            //Insert block changes @ 1
            let mut block_changes = BlockChanges {
                block_num: 1,
                block_hash: B256::from([0; 32]),
                state_changes: Default::default(),
            };

            db.commit_block(FsCommitBlockParams {
                block_changes: block_changes.clone(),
                ..Default::default()
            })
            .unwrap();
            let block_changes_tree = db.block_changes_tree().unwrap();

            let block_changes_db = block_changes_tree.get(1.serialize()).unwrap().unwrap();

            assert_eq!(
                block_changes,
                BlockChanges::deserialize(block_changes_db).unwrap()
            );

            let block_changes_to_remove = vec![block_changes.block_num];

            //Insert block changes @ 2
            block_changes.block_num = 2;

            db.commit_block(FsCommitBlockParams {
                block_changes: block_changes.clone(),
                block_changes_to_remove,
                ..Default::default()
            })
            .unwrap();

            //Assert block 2 is inserted and block 1 is pruned
            let block_changes_db = block_changes_tree.get(2.serialize()).unwrap().unwrap();

            assert_eq!(
                block_changes,
                BlockChanges::deserialize(block_changes_db).unwrap()
            );

            let block_changes_db = block_changes_tree.get(1.serialize()).unwrap();
            assert!(block_changes_db.is_none());
            assert_eq!(block_changes_tree.len(), 1);
        }

        #[test]
        fn test_insert_code_hashes() {
            let db = FsDb::new_test();

            let code_hashes_to_insert = vec![
                (
                    random_bytes(),
                    Bytecode::new_raw(random_bytes::<22_000>().into()),
                ),
                (
                    random_bytes(),
                    Bytecode::new_raw(random_bytes::<65>().into()),
                ),
            ];

            db.commit_block(FsCommitBlockParams {
                code_hashes_to_insert: code_hashes_to_insert.clone(),
                ..Default::default()
            })
            .unwrap();

            let code_by_hash_tree = db.code_by_hash_tree().unwrap();

            for (code_hash, bytecode) in code_hashes_to_insert {
                let code_db = code_by_hash_tree
                    .get(code_hash.0.to_vec().serialize())
                    .unwrap()
                    .unwrap();

                assert_eq!(bytecode, Bytecode::deserialize(code_db).unwrap());
            }
        }

        #[test]
        fn test_storage_val_history() {
            let db = FsDb::new_test();

            // Create test data
            let storage_slot_key = StorageSlotKey {
                address: random_bytes().into(),
                slot: random_bytes().into(),
            };

            let mut value_history = ValueHistory::default();
            value_history.insert(1, random_bytes().into());
            value_history.insert(2, random_bytes().into());

            // Commit the storage value history
            db.commit_block(FsCommitBlockParams {
                storage_value_histories: vec![StorageValueHistory {
                    key: storage_slot_key.clone(),
                    value_history: value_history.clone(),
                }],
                ..Default::default()
            })
            .unwrap();

            // Verify the storage value history was stored correctly
            let storage_tree = db.storage_tree().unwrap();
            let stored_history = storage_tree
                .get(storage_slot_key.serialize())
                .unwrap()
                .unwrap();

            assert_eq!(
                value_history,
                ValueHistory::<U256>::deserialize(stored_history).unwrap()
            );
        }

        //Test storage value histories when update is empty or only contains a zero value
        #[test]
        fn test_storage_val_history_removal() {
            let db = FsDb::new_test();

            let mut value_history = ValueHistory::default();
            value_history.insert(1, U256::ZERO);
            for value_history in [ValueHistory::default(), value_history] {
                // Create test data
                let storage_slot_key = StorageSlotKey {
                    address: random_bytes().into(),
                    slot: random_bytes().into(),
                };

                let mut value_history_init = ValueHistory::default();
                value_history_init.insert(1, random_bytes().into());

                // Commit the storage value history
                db.commit_block(FsCommitBlockParams {
                    storage_value_histories: vec![StorageValueHistory {
                        key: storage_slot_key.clone(),
                        value_history: value_history_init,
                    }],
                    ..Default::default()
                })
                .unwrap();

                // Handle the reorg
                db.handle_reorg(FsHandleReorgParams {
                    storage_value_histories: vec![StorageValueHistory {
                        key: storage_slot_key.clone(),
                        value_history: value_history.clone(),
                    }],
                    ..Default::default()
                })
                .unwrap();

                let storage_tree = db.storage_tree().unwrap();
                // Verify the storage value history was stored correctly
                assert!(storage_tree
                    .get(storage_slot_key.serialize())
                    .unwrap()
                    .is_none());
            }
        }

        #[test]
        fn test_account_val_history() {
            let db = FsDb::new_test();

            // Create test data
            let address: Address = random_bytes().into();
            let account_info_0 = AccountInfo {
                code: None,
                nonce: 1,
                balance: random_bytes().into(),
                code_hash: random_bytes(),
            };
            let account_info_1 = AccountInfo {
                code: Some(Bytecode::new_raw(Bytes::from(random_bytes::<22_000>()))),
                nonce: 1,
                balance: random_bytes().into(),
                code_hash: random_bytes(),
            };
            let mut value_history = ValueHistory::default();
            value_history.insert(1, account_info_0.clone());
            value_history.insert(2, account_info_1.clone());

            // Commit the account value history
            db.commit_block(FsCommitBlockParams {
                account_value_histories: vec![AccountValueHistory {
                    address,
                    value_history: value_history.clone(),
                }],
                ..Default::default()
            })
            .unwrap();

            // Verify the account value history was stored correctly
            let basic_tree = db.basic_tree().unwrap();
            let stored_history = basic_tree.get(address.serialize()).unwrap().unwrap();

            assert_eq!(
                value_history,
                ValueHistory::<AccountInfo>::deserialize(stored_history).unwrap()
            );
        }
    }
}

#[cfg(test)]
mod test_fsdb_io {
    use super::*;
    use crate::{
        primitives::{
            Bytes,
            ValueHistory,
        },
        test_utils::random_bytes,
    };
    use std::collections::HashMap;

    fn create_test_account_info() -> AccountInfo {
        AccountInfo {
            nonce: 1,
            balance: U256::from(1000),
            code_hash: random_bytes(),
            code: Some(Bytecode::new_raw(Bytes::from(random_bytes::<64>()))),
        }
    }

    mod test_write {
        use super::*;

        #[test]
        fn test_write_storage() -> Result<(), FsDbError> {
            let fs_db = FsDb::new_test();
            let mut mem_db = MemoryDb::<5>::default();

            // Create test storage data
            let address1: Address = random_bytes().into();
            let address2: Address = random_bytes().into();

            let mut storage1 = HashMap::new();
            let mut value_history1 = ValueHistory::new();
            value_history1.insert(1, U256::from(100));
            value_history1.insert(2, U256::from(200));
            storage1.insert(U256::from(1), value_history1);

            let mut storage2 = HashMap::new();
            let mut value_history2 = ValueHistory::new();
            value_history2.insert(1, U256::from(300));
            value_history2.insert(2, U256::from(400));
            storage2.insert(U256::from(2), value_history2);

            mem_db.storage.insert(address1, storage1);
            mem_db.storage.insert(address2, storage2);

            // Verify write succeeds
            assert!(fs_db.write_storage_to_tree(&mem_db).is_ok());

            // Test writing empty storage
            let empty_mem_db = MemoryDb::<5>::default();
            assert!(fs_db.write_storage_to_tree(&empty_mem_db).is_ok());

            Ok(())
        }

        #[test]
        fn test_write_basic() -> Result<(), FsDbError> {
            let fs_db = FsDb::new_test();
            let mut mem_db = MemoryDb::<5>::default();

            // Create test basic account data
            let address1: Address = random_bytes().into();
            let address2: Address = random_bytes().into();

            let mut history1 = ValueHistory::new();
            history1.insert(1, create_test_account_info());

            let mut history2 = ValueHistory::new();
            history2.insert(2, create_test_account_info());

            mem_db.basic.insert(address1, history1);
            mem_db.basic.insert(address2, history2);

            // Verify write succeeds
            assert!(fs_db.write_basic_to_tree(&mem_db).is_ok());

            // Test writing account with no code
            let mut no_code_history = ValueHistory::new();
            let mut no_code_account = create_test_account_info();
            no_code_account.code = None;
            no_code_history.insert(1, no_code_account);

            mem_db.basic.clear();
            mem_db.basic.insert(random_bytes().into(), no_code_history);

            assert!(fs_db.write_basic_to_tree(&mem_db).is_ok());

            Ok(())
        }

        #[test]
        fn test_write_block_hashes() -> Result<(), FsDbError> {
            let fs_db = FsDb::new_test();
            let mut mem_db = MemoryDb::<5>::default();

            // Create test block hash data
            mem_db.block_hashes.insert(1, random_bytes());
            mem_db.block_hashes.insert(2, random_bytes());
            mem_db.block_hashes.insert(3, random_bytes());
            mem_db.canonical_block_num = 3;
            mem_db.canonical_block_hash = mem_db.block_hashes[&3];

            // Verify write succeeds
            assert!(fs_db.write_block_hashes_to_tree(&mem_db).is_ok());

            // Test writing empty block hashes
            let empty_mem_db = MemoryDb::<5>::default();
            assert!(fs_db.write_block_hashes_to_tree(&empty_mem_db).is_ok());

            // Test writing non-sequential blocks
            mem_db.block_hashes.clear();
            mem_db.block_hashes.insert(1, random_bytes());
            mem_db.block_hashes.insert(5, random_bytes());
            mem_db.block_hashes.insert(10, random_bytes());

            assert!(fs_db.write_block_hashes_to_tree(&mem_db).is_ok());

            Ok(())
        }

        #[test]
        fn test_write_code_by_hash() -> Result<(), FsDbError> {
            let fs_db = FsDb::new_test();
            let mut mem_db = MemoryDb::<5>::default();

            // Create test bytecode data
            let code_hash1 = random_bytes();
            let code_hash2 = random_bytes();

            let bytecode1 = Bytecode::new_raw(Bytes::from(random_bytes::<128>()));
            let bytecode2 = Bytecode::new_raw(Bytes::from(random_bytes::<256>()));

            mem_db.code_by_hash.insert(code_hash1, bytecode1);
            mem_db.code_by_hash.insert(code_hash2, bytecode2);

            // Verify write succeeds
            assert!(fs_db.write_code_by_hash_to_tree(&mem_db).is_ok());

            // Test writing empty code
            let empty_mem_db = MemoryDb::<5>::default();
            assert!(fs_db.write_code_by_hash_to_tree(&empty_mem_db).is_ok());

            // Test writing large bytecode
            let large_bytecode = Bytecode::new_raw(Bytes::from(random_bytes::<100_000>()));
            mem_db.code_by_hash.clear();
            mem_db.code_by_hash.insert(random_bytes(), large_bytecode);

            assert!(fs_db.write_code_by_hash_to_tree(&mem_db).is_ok());

            Ok(())
        }
    }

    mod test_load {
        use super::*;

        #[test]
        fn test_load_storage() -> Result<(), FsDbError> {
            let fs_db = FsDb::new_test();
            let mut mem_db = MemoryDb::<5>::default();

            // Set up test data
            let address1: Address = random_bytes().into();
            let mut storage1 = HashMap::new();
            let mut value_history1 = ValueHistory::new();
            value_history1.insert(1, U256::from(100));
            storage1.insert(U256::from(1), value_history1);
            mem_db.storage.insert(address1, storage1.clone());

            fs_db.write_storage_to_tree(&mem_db)?;

            // Test loading data
            let loaded_storage = fs_db.load_storage()?;
            assert_eq!(loaded_storage.len(), mem_db.storage.len());
            assert_eq!(loaded_storage.get(&address1), mem_db.storage.get(&address1));

            // Test loading empty storage
            let loaded_pre = fs_db.load_storage()?;
            fs_db.write_storage_to_tree(&MemoryDb::<5>::default())?;
            let loaded_empty = fs_db.load_storage()?;
            assert_eq!(loaded_empty, loaded_pre);

            Ok(())
        }

        #[test]
        fn test_load_basic() -> Result<(), FsDbError> {
            let fs_db = FsDb::new_test();
            let mut mem_db = MemoryDb::<5>::default();

            // Set up test data
            let address = random_bytes().into();
            let mut history = ValueHistory::new();
            history.insert(1, create_test_account_info());
            mem_db.basic.insert(address, history.clone());

            fs_db.write_basic_to_tree(&mem_db)?;

            // Test loading data
            let loaded_basic = fs_db.load_basic()?;
            assert_eq!(loaded_basic.len(), mem_db.basic.len());
            assert_eq!(loaded_basic.get(&address), Some(&history));

            // Test loading account with no code
            let mut no_code_account = create_test_account_info();
            no_code_account.code = None;
            let mut no_code_history = ValueHistory::new();
            no_code_history.insert(1, no_code_account.clone());
            mem_db.basic.clear();
            mem_db.basic.insert(address, no_code_history);

            fs_db.write_basic_to_tree(&mem_db)?;
            let loaded_no_code = fs_db.load_basic()?;
            assert_eq!(
                loaded_no_code
                    .get(&address)
                    .unwrap()
                    .get_latest()
                    .unwrap()
                    .code,
                None
            );

            Ok(())
        }

        #[test]
        fn test_load_block_hashes() -> Result<(), FsDbError> {
            let fs_db = FsDb::new_test();
            let mut mem_db = MemoryDb::<5>::default();

            // Set up test data
            let block_hash = random_bytes();
            mem_db.block_hashes.insert(1, block_hash);
            mem_db.canonical_block_num = 1;
            mem_db.canonical_block_hash = block_hash;

            fs_db.write_block_hashes_to_tree(&mem_db)?;

            // Test loading data
            let (loaded_hashes, loaded_num, loaded_hash) = fs_db.load_block_hashes()?;
            assert_eq!(loaded_hashes, mem_db.block_hashes);
            assert_eq!(loaded_num, mem_db.canonical_block_num);
            assert_eq!(loaded_hash, mem_db.canonical_block_hash);

            // Test loading empty state
            let (loaded_pre, _, _) = fs_db.load_block_hashes()?;
            fs_db.write_block_hashes_to_tree(&MemoryDb::<5>::default())?;
            let (loaded_empty, num, hash) = fs_db.load_block_hashes()?;
            assert_eq!(loaded_empty, loaded_pre);
            assert_eq!(num, 1);
            assert_eq!(hash, block_hash);

            Ok(())
        }

        #[test]
        fn test_load_code_by_hash() -> Result<(), FsDbError> {
            let fs_db = FsDb::new_test();
            let mut mem_db = MemoryDb::<5>::default();

            // Set up test data
            let code_hash = random_bytes();
            let bytecode = Bytecode::new_raw(Bytes::from(random_bytes::<128>()));
            mem_db.code_by_hash.insert(code_hash, bytecode.clone());

            fs_db.write_code_by_hash_to_tree(&mem_db)?;

            // Test loading data
            let loaded_code = fs_db.load_code_by_hash()?;
            assert_eq!(loaded_code.len(), mem_db.code_by_hash.len());
            assert_eq!(
                loaded_code.get(&code_hash).unwrap().bytes_slice(),
                bytecode.bytes_slice()
            );

            // Test loading empty code
            let loaded_pre = fs_db.load_code_by_hash()?;
            fs_db.write_code_by_hash_to_tree(&MemoryDb::<5>::default())?;
            let loaded_empty = fs_db.load_code_by_hash()?;
            assert_eq!(loaded_empty, loaded_pre);

            Ok(())
        }

        #[test]
        fn test_load_persistence() -> Result<(), FsDbError> {
            // Create temp directory that will persist for test duration
            let temp_dir = tempfile::tempdir()?;
            let fs_db = FsDb::new(temp_dir.path())?;
            let mut mem_db = MemoryDb::<5>::default();

            // Create and write test data
            let address = random_bytes().into();
            let mut history = ValueHistory::new();
            history.insert(1, create_test_account_info());
            mem_db.basic.insert(address, history.clone());

            fs_db.write_basic_to_tree(&mem_db)?;
            drop(fs_db);

            // Create new instance and verify data persists
            let new_fs_db = FsDb::new(temp_dir.path())?;
            let loaded_basic = new_fs_db.load_basic()?;
            assert_eq!(loaded_basic.get(&address), Some(&history));

            Ok(())
        }
    }
}
