use crate::primitives::{
    AccountInfo,
    Address,
    BlockChanges,
    Bytecode,
    ValueHistory,
    B256,
    U256,
};

use super::{
    serde::{
        Serialize,
        StorageSlotKey,
    },
    FsDbError,
};

use std::path::Path;

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
                    address: address.clone(),
                    value_history: value_history.clone(),
                }],

                ..Default::default()
            })
            .unwrap();

            db.handle_reorg(FsHandleReorgParams {
                account_value_histories: vec![AccountValueHistory {
                    address: address.clone(),
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

            let block_changes_to_remove = vec![block_changes.block_num.clone()];

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
                    address: address.clone(),
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
