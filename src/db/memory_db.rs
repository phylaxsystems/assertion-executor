use crate::{
    db::{
        fs::fs_db::{
            AccountValueHistory,
            FsCommitBlockParams,
            FsHandleReorgParams,
            StorageValueHistory,
        },
        DatabaseRef,
        NotFoundError,
    },
    primitives::{
        AccountInfo,
        Address,
        BlockChanges,
        Bytecode,
        EvmStorageSlot,
        TouchedKeys,
        ValueHistory,
        B256,
        U256,
    },
};

use std::collections::{
    hash_map::Entry,
    BTreeMap,
    HashMap,
    VecDeque,
};

use reth_primitives_traits::{
    Account as RethAccount,
    Bytecode as RethBytecode,
};

use reth_db::table::{
    Decompress,
    Table,
};

use revm::primitives::FixedBytes;
use sled::Db as SledDb;

use thiserror::Error;

use bincode;
use serde::{
    Deserialize,
    Serialize,
};

/// Struct for serializing/deserializng PST table values.
#[derive(Serialize, Deserialize, Debug)]
pub struct StorageMap(pub HashMap<Vec<u8>, Vec<u8>>);

/// Type alias for the `MemoryDb` EVM storage struct.
type EvmStorage = HashMap<Address, HashMap<U256, ValueHistory<U256>>>;

#[derive(Error, Debug)]
pub enum MemoryDbError {
    #[error("Failed to load MemoryDb from sled!")]
    LoadFail,
    #[error("Array conversion error: {0}")]
    ArrayConversionError(#[from] std::array::TryFromSliceError),
    #[error("Database error: {0}")]
    DatabaseError(#[from] reth_db::DatabaseError),
    #[error("Bincode deserialization error: {0}")]
    BincodeError(#[from] Box<bincode::ErrorKind>),
    #[error("EoF Decoder error")]
    EofDecoderError,
    #[error("Bytecode Hash Conversion Error")]
    BytecodeHashConversionError,
}

/// Contains current and past state of an EVM chain.
#[derive(Debug, Clone, Default)]
pub struct MemoryDb<const BLOCKS_TO_RETAIN: usize> {
    /// Maps addresses to storage slots and their history indexed by block.
    pub(super) storage: EvmStorage,
    /// Maps addresses to their account info and indexes it by block.
    pub(super) basic: HashMap<Address, ValueHistory<AccountInfo>>,
    /// Maps block numbers to block hashes.
    pub(super) block_hashes: BTreeMap<u64, B256>,
    /// Maps bytecode hashes to bytecode.
    pub(super) code_by_hash: HashMap<B256, Bytecode>,
    /// Hash of the head block.
    pub(super) canonical_block_hash: B256,
    /// Block number of the head block.
    pub(super) canonical_block_num: u64,
    /// Block Changes for the blocks that have not been pruned.
    pub(super) block_changes: VecDeque<BlockChanges>,
}

impl<const BLOCKS_TO_RETAIN: usize> MemoryDb<BLOCKS_TO_RETAIN> {
    /// Iterates over all the KV pairs and loads them into the corresponding struct.
    fn load_tables_into_db<const LEAF_FANOUT: usize>(
        &mut self,
        sled_db: &SledDb<LEAF_FANOUT>,
    ) -> Result<(), MemoryDbError> {
        // Define which tables we're extracting
        //
        // Note: Do not change the order of this array
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

        for table_name in reth_tables {
            match *table_name {
                // <Hash, Blocknumber>
                // Loads `block_hashes` as well as the `canonical_block_hash` and `canonical_block_num`
                // NOTE: this table must be deserialized first
                "HeaderNumbers" => {
                    let result = self.load_header_numbers(sled_db, table_name)?;
                    canonical_block_hash = result.0;
                    canonical_block_num = result.1;
                }
                // <Code hash, Bytecode>
                // Loads `code_by_hash` and supplements `basic`
                "Bytecodes" => self.load_bytecodes(sled_db, table_name)?,
                // <Address, Account>
                // Loads `basic`
                // NEEDS `Bytecodes` to be completed before this
                "PlainAccountState" => {
                    self.load_plain_account_state(sled_db, table_name, canonical_block_num)?
                }
                // <Address, Hashmap<slot, value>>
                // Loads `storage`
                // NEEDS `HeaderNumbers` to be completed before this.
                "PlainStorageState" => {
                    self.load_plain_storage_state(sled_db, table_name, canonical_block_num)?
                }
                _ => return Err(MemoryDbError::LoadFail),
            }
        }

        self.canonical_block_hash = canonical_block_hash;
        self.canonical_block_num = canonical_block_num;

        Ok(())
    }

    fn load_header_numbers<const LEAF_FANOUT: usize>(
        &mut self,
        sled_db: &SledDb<LEAF_FANOUT>,
        table_name: &str,
    ) -> Result<(B256, u64), MemoryDbError> {
        let sled_table = sled_db
            .open_tree(table_name)
            .map_err(|_| MemoryDbError::LoadFail)?;
        let mut canonical_block_hash = Default::default();
        let mut canonical_block_num = 0;

        for item in sled_table.iter() {
            if let Ok((key, value)) = item {
                let key: B256 = B256::new(key.as_ref().try_into()?);

                let value: &[u8] = value.as_ref();
                let value: u64 = u64::from_le_bytes(value.try_into()?);

                // check if we have a higher block number
                if canonical_block_num < value {
                    canonical_block_num = value;
                    canonical_block_hash = key;
                }

                self.block_hashes.insert(value, key);
            }
        }

        Ok((canonical_block_hash, canonical_block_num))
    }

    fn load_bytecodes<const LEAF_FANOUT: usize>(
        &mut self,
        sled_db: &SledDb<LEAF_FANOUT>,
        table_name: &str,
    ) -> Result<(), MemoryDbError> {
        let sled_table = sled_db
            .open_tree(table_name)
            .map_err(|_| MemoryDbError::LoadFail)?;

        for item in sled_table.iter() {
            if let Ok((key, value)) = item {
                let key: B256 = B256::new(key.as_ref().try_into()?);

                let value: Vec<u8> = value.as_ref().into();
                let value: RethBytecode = RethBytecode::decompress(&value)?;

                // Now convert this to revm `Bytecode`
                // absolutely insane typing
                let bytecode: Bytecode =
                    Bytecode::new_raw_checked(value.0.bytecode().to_vec().into())
                        .map_err(|_| MemoryDbError::EofDecoderError)?;

                // Insert into the memory_db
                self.code_by_hash.insert(key, bytecode);
            }
        }

        Ok(())
    }

    fn load_plain_account_state<const LEAF_FANOUT: usize>(
        &mut self,
        sled_db: &SledDb<LEAF_FANOUT>,
        table_name: &str,
        canonical_block_num: u64,
    ) -> Result<(), MemoryDbError> {
        let sled_table = sled_db
            .open_tree(table_name)
            .map_err(|_| MemoryDbError::LoadFail)?;

        for item in sled_table.iter() {
            if let Ok((key, value)) = item {
                let key: Address = Address::new(key.as_ref().try_into()?);

                let value: reth_primitives_traits::Account = RethAccount::decompress(&value)?;
                let value: <reth_db::PlainAccountState as Table>::Value = value;

                // We have to convert `PlainAccountState` to `AccountInfo`
                // `AccountInfo` contains some additional data we need that is not

                // Get the code by addressing the `code_by_hash` field of `MemoryDb`.
                let (code_hash, code) = self.get_code_info(&value);

                let account_info: AccountInfo = AccountInfo {
                    nonce: value.nonce,
                    balance: value.balance,
                    code_hash,
                    code,
                };

                let mut history = ValueHistory::new();
                history.insert(canonical_block_num, account_info);
                self.basic.insert(key, history);
            }
        }

        Ok(())
    }

    fn get_code_info(&self, value: &RethAccount) -> (FixedBytes<32>, Option<Bytecode>) {
        let mut code_hash: FixedBytes<32> = (&[0; 32]).into();
        let mut code: Option<Bytecode> = None;

        if let Some(bytecode_hash) = value.bytecode_hash {
            let bytecode_hash: FixedBytes<32> = bytecode_hash
                .as_slice()
                .try_into()
                .map_err(|_| MemoryDbError::BytecodeHashConversionError)
                .unwrap();

            code_hash = bytecode_hash;

            // If we have the bytecode hash, we can get the bytecode
            if let Some(bytecode) = self.code_by_hash.get(&bytecode_hash) {
                code = Some(bytecode.clone());
            }
        }

        (code_hash, code)
    }

    fn load_plain_storage_state<const LEAF_FANOUT: usize>(
        &mut self,
        sled_db: &SledDb<LEAF_FANOUT>,
        table_name: &str,
        canonical_block_num: u64,
    ) -> Result<(), MemoryDbError> {
        let sled_table = sled_db
            .open_tree(table_name)
            .map_err(|_| MemoryDbError::LoadFail)?;

        for item in sled_table.iter() {
            if let Ok((key, value)) = item {
                // Get the address
                let key: Address = Address::new(key.as_ref().try_into()?);

                // Deserialize the StorageMap
                let storage_map: StorageMap = bincode::deserialize(&value)?;

                // Create a new HashMap for this address in the storage
                let address_storage = self.storage.entry(key).or_default();

                // Iterate over the storage slots and values
                for (slot_bytes, value_bytes) in storage_map.0 {
                    let slot: FixedBytes<32> = slot_bytes.as_slice().try_into()?;
                    let slot: U256 = slot.into();

                    let value: FixedBytes<32> = value_bytes.as_slice().try_into()?;
                    let value: U256 = value.into();

                    // Create a new ValueHistory for this slot
                    let mut history = ValueHistory::new();
                    history.insert(canonical_block_num, value);

                    // Insert the history into the address storage
                    address_storage.insert(slot, history);
                }
            }
        }

        Ok(())
    }

    /// Opens a `sled` Db that has had exported and transformed data from `reth` MDBX with
    /// `mending::transform_reth_tables` and loads it into itself.
    pub fn load_from_exported_sled(&mut self, sled_db: &SledDb) -> Result<(), MemoryDbError> {
        self.load_tables_into_db(sled_db)?;
        Ok(())
    }
}

/// Opens a `sled` Db that has had exported and transformed data from `reth` MDBX with
/// `mending::transform_reth_tables`.
pub fn new_from_exported_sled<const LEAF_FANOUT: usize, const BLOCKS_TO_RETAIN: usize>(
    sled_db: &SledDb<LEAF_FANOUT>,
) -> Result<MemoryDb<BLOCKS_TO_RETAIN>, MemoryDbError> {
    let mut memory_db = MemoryDb::default();
    memory_db.load_tables_into_db::<LEAF_FANOUT>(sled_db)?;
    Ok(memory_db)
}

impl<const BLOCKS_TO_RETAIN: usize> MemoryDb<BLOCKS_TO_RETAIN> {
    ///Handles reorgs.
    ///This function will take the new head block hash and reorg the db to that block.
    ///Reorging will prune -
    ///value histories of accounts and storage slots that have changed
    ///Block hashes for blocks that are greater than the block number of the reorged block
    ///
    ///This function will remove all block changes from the block number of the reorged block
    ///Returns the FsHandleReorgParams which contains the values needed to update the
    ///[`crate::db::fs::FsDb`]
    pub fn handle_reorg(&mut self, block_hash: B256) -> Option<FsHandleReorgParams> {
        let (index, block_num) = if let Some((index, BlockChanges { block_num, .. })) =
            &self.block_changes.iter().enumerate().rev().find(
                |(
                    _,
                    BlockChanges {
                        block_hash: block_hash_i,
                        ..
                    },
                )| block_hash.eq(block_hash_i),
            ) {
            (*index, *block_num)
        } else {
            //TODO: block hash not found, possibly a block that the executor has not seen yet or
            //has already pruned.
            //Perhaps we request the block from the rpc and try to reconcile the db.
            return None;
        };

        let block_numbers_to_remove = self
            .block_changes
            .range(index + 1..)
            .map(|block_change| block_change.block_num)
            .collect();

        let removed_block_changes = self.block_changes.split_off(index + 1);

        let touched_keys = removed_block_changes.iter().fold(
            TouchedKeys::default(),
            |mut touched_keys, block_change| {
                touched_keys.extend(block_change.touched_keys());
                touched_keys
            },
        );

        self.prune_from(&touched_keys, block_num);
        self.set_canonical_block(block_num, block_hash);
        let (account_value_histories, storage_value_histories) =
            self.get_touched_val_histories(&touched_keys);

        Some(FsHandleReorgParams {
            block_numbers_to_remove,
            account_value_histories,
            storage_value_histories,
        })
    }

    pub fn commit_block(&mut self, block_changes: BlockChanges) -> FsCommitBlockParams {
        self.set_canonical_block(block_changes.block_num, block_changes.block_hash);

        let mut block_changes = block_changes;

        let mut code_hashes_to_insert = Vec::new();
        for (address, account) in block_changes.state_changes.iter_mut() {
            if account.is_selfdestructed() {
                self.basic.remove(address);
                if let Some(storage_history) = self.storage.get_mut(address) {
                    for (slot, val_history) in storage_history.iter_mut() {
                        account.storage.insert(
                            *slot,
                            EvmStorageSlot {
                                present_value: U256::ZERO,
                                original_value: *val_history.get_latest().unwrap_or(&U256::ZERO),
                                is_cold: false,
                            },
                        );
                        val_history.insert(self.canonical_block_num, U256::ZERO);
                    }
                }
                continue;
            }

            if let Some(ref code) = account.info.code {
                self.code_by_hash
                    .insert(account.info.code_hash, code.clone());
                code_hashes_to_insert.push((account.info.code_hash, code.clone()));

                let contract_storage = self.storage.entry(*address).or_default();
                for (slot, evm_storage_slot) in account.storage.clone() {
                    contract_storage
                        .entry(slot)
                        .or_default()
                        .insert(self.canonical_block_num, evm_storage_slot.present_value());
                }
            }

            self.basic
                .entry(*address)
                .or_default()
                .insert(self.canonical_block_num, account.info.clone());
        }

        self.block_changes.push_back(block_changes.clone());
        self.block_hashes
            .insert(block_changes.block_num, block_changes.block_hash);

        //Remove the pruneable block changes
        let pruneable_block_num = block_changes.block_num.checked_sub(BLOCKS_TO_RETAIN as u64);

        let mut block_changes_to_remove = Vec::new();
        if let Some(pruneable_block_num) = pruneable_block_num {
            while let Some(block_change) = self.block_changes.front() {
                if block_change.block_num <= pruneable_block_num {
                    block_changes_to_remove.push(block_change.block_num);
                    self.block_changes.pop_front();
                } else {
                    break;
                }
            }
        }

        let (account_value_histories, storage_value_histories) =
            self.get_touched_val_histories(&block_changes.touched_keys());

        FsCommitBlockParams {
            block_changes,
            block_changes_to_remove,
            code_hashes_to_insert,
            account_value_histories,
            storage_value_histories,
        }
    }

    ///Prune the database from the block number of the reorged block to the current block number.
    ///This function will remove all value history entries of accounts and storage slots that have
    ///changed, and block hashes for blocks that are greater than the block number of the reorged
    fn prune_from(&mut self, touched_keys: &TouchedKeys, block_num: u64) {
        for key in &touched_keys.basic {
            if let Entry::Occupied(mut v) = self.basic.entry(*key) {
                let val_history = v.get_mut();
                val_history.prune_from(block_num);
                if val_history.is_empty() {
                    v.remove_entry();
                }
            }
        }

        for key in &touched_keys.storage {
            if let Entry::Occupied(mut s) = self.storage.entry(key.address) {
                if let Entry::Occupied(mut v) = s.get_mut().entry(key.slot) {
                    let val_history = v.get_mut();
                    val_history.prune_from(block_num);
                    if val_history.is_empty()
                        || (val_history.len() == 1
                            && val_history.get_latest().unwrap() == &U256::ZERO)
                    {
                        v.remove_entry();
                        if s.get().is_empty() {
                            s.remove_entry();
                        }
                    }
                }
            };
        }

        self.block_hashes.split_off(&(block_num + 1));
    }

    ///Set the canonical block number and hash
    fn set_canonical_block(&mut self, block_num: u64, block_hash: B256) {
        self.canonical_block_num = block_num;
        self.canonical_block_hash = block_hash;
    }

    fn get_touched_val_histories(
        &self,
        touched_keys: &TouchedKeys,
    ) -> (Vec<AccountValueHistory>, Vec<StorageValueHistory>) {
        let mut touched_val_histories_basic = Vec::new();
        for address in &touched_keys.basic {
            let value_history = self.basic.get(address).cloned().unwrap_or_default();
            touched_val_histories_basic.push(AccountValueHistory {
                address: *address,
                value_history,
            });
        }
        let mut touched_val_histories_storage = Vec::new();
        for key in &touched_keys.storage {
            let value_history = match self.storage.get(&key.address) {
                Some(storage) => storage.get(&key.slot).cloned().unwrap_or_default(),
                None => Default::default(),
            };

            touched_val_histories_storage.push(StorageValueHistory {
                key: key.clone(),
                value_history,
            });
        }
        (touched_val_histories_basic, touched_val_histories_storage)
    }
}

impl<const BLOCKS_TO_RETAIN: usize> DatabaseRef for MemoryDb<BLOCKS_TO_RETAIN> {
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
    fn storage_ref(&self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        Ok(*self
            .storage
            .get(&address)
            .and_then(|s| s.get(&slot).map(|v| v.get_latest().unwrap_or(&U256::ZERO)))
            .unwrap_or(&U256::ZERO))
    }
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash
            .get(&code_hash)
            .cloned()
            .ok_or(NotFoundError)
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::{
        primitives::{
            Account,
            AccountStatus,
            Address,
            Bytes,
        },
        test_utils::random_bytes,
    };
    use std::collections::HashMap;

    #[cfg(test)]
    mod memory_db_insert_tests {
        use super::*;
        use alloy::primitives::{
            B256,
            U256,
        };
        use rand::{
            thread_rng,
            Rng,
        };
        use reth_db::table::Compress;
        use reth_primitives_traits::{
            Account as RethAccount,
            Bytecode as RethBytecode,
            StorageEntry,
        };

        fn create_mock_sled_db() -> sled::Db {
            let temp_dir = tempfile::tempdir().unwrap();
            sled::Config::new().path(temp_dir).open().unwrap()
        }

        fn random_b256() -> B256 {
            let mut rng = thread_rng();
            let mut bytes = [0u8; 32];
            rng.fill(&mut bytes);
            B256::from(bytes)
        }

        fn random_address() -> Address {
            let mut rng = thread_rng();
            let mut bytes = [0u8; 20];
            rng.fill(&mut bytes);
            Address::from(bytes)
        }

        #[test]
        fn test_insert_header_numbers() -> Result<(), Box<dyn std::error::Error>> {
            let sled_db = create_mock_sled_db();
            let header_numbers = sled_db.open_tree("HeaderNumbers")?;

            let block_hash1 = random_b256();
            let block_number1 = 1u64;
            let block_hash2 = random_b256();
            let block_number2 = 2u64;

            header_numbers.insert(block_hash1.as_slice(), &block_number1.to_le_bytes())?;
            header_numbers.insert(block_hash2.as_slice(), &block_number2.to_le_bytes())?;

            let memory_db = new_from_exported_sled::<1024, 5>(&sled_db)?;

            assert_eq!(
                memory_db.block_hashes.get(&block_number1).unwrap(),
                block_hash1.as_slice()
            );
            assert_eq!(
                memory_db.block_hashes.get(&block_number2).unwrap(),
                block_hash2.as_slice()
            );
            assert_eq!(memory_db.canonical_block_hash, *block_hash2);
            assert_eq!(memory_db.canonical_block_num, block_number2);

            Ok(())
        }

        #[test]
        fn test_insert_bytecodes() -> Result<(), Box<dyn std::error::Error>> {
            let sled_db = create_mock_sled_db();
            let bytecodes = sled_db.open_tree("Bytecodes")?;

            let code_hash = random_b256();
            let bytecode = RethBytecode::new_raw(vec![1, 2, 3].into());
            bytecodes.insert(code_hash.as_slice(), bytecode.clone().compress())?;

            let memory_db = new_from_exported_sled::<1024, 5>(&sled_db)?;

            assert!(memory_db
                .code_by_hash
                .contains_key::<FixedBytes<32>>(&code_hash.as_slice().try_into()?));

            assert_eq!(
                memory_db
                    .code_by_hash
                    .get::<FixedBytes<32>>(&code_hash.as_slice().try_into()?)
                    .unwrap()
                    .original_byte_slice(),
                bytecode.0.bytecode()
            );

            Ok(())
        }

        #[test]
        fn test_insert_plain_account_state() -> Result<(), Box<dyn std::error::Error>> {
            let sled_db = create_mock_sled_db();
            let plain_account_state = sled_db.open_tree("PlainAccountState")?;
            let bytecodes = sled_db.open_tree("Bytecodes")?;
            let header_numbers = sled_db.open_tree("HeaderNumbers")?;

            let address = random_address();
            let account = RethAccount {
                nonce: 1,
                balance: U256::from(100),
                bytecode_hash: Some(random_b256()),
            };
            plain_account_state.insert(address.as_slice(), account.compress())?;

            // Insert a dummy block number to set the canonical block
            let block_hash = random_b256();
            let block_number = 1u64;
            header_numbers.insert(block_hash.as_slice(), &block_number.to_le_bytes())?;

            // Insert corresponding bytecode if account has bytecode_hash
            if let Some(bytecode_hash) = account.bytecode_hash {
                let bytecode = RethBytecode::new_raw(vec![1, 2, 3].into());
                bytecodes.insert(bytecode_hash.as_slice(), bytecode.compress())?;
            }

            let memory_db = new_from_exported_sled::<1024, 5>(&sled_db)?;

            assert!(memory_db
                .basic
                .contains_key::<Address>(&Address::from_slice(address.as_slice())));

            let stored_account = memory_db
                .basic
                .get(&Address::from_slice(address.as_slice()))
                .unwrap()
                .get_latest()
                .unwrap();
            assert_eq!(stored_account.nonce, account.nonce);
            assert_eq!(stored_account.balance, account.balance);
            assert_eq!(
                stored_account.code_hash,
                *account.bytecode_hash.unwrap_or_default()
            );

            Ok(())
        }

        #[test]
        fn test_insert_plain_storage_state() -> Result<(), Box<dyn std::error::Error>> {
            let sled_db = create_mock_sled_db();
            let plain_storage_state = sled_db.open_tree("PlainStorageState")?;
            let header_numbers = sled_db.open_tree("HeaderNumbers")?;

            let address = random_address();
            let storage_key = random_b256();
            let storage_value = U256::from(42);
            let _storage_entry = StorageEntry {
                key: storage_key,
                value: storage_value,
            };

            let mut storage_map = HashMap::new();
            storage_map.insert(
                storage_key.to_vec(),
                storage_value.to_be_bytes::<32>().to_vec(),
            );
            let serialized_map = bincode::serialize(&StorageMap(storage_map))?;
            plain_storage_state.insert(address.as_slice(), serialized_map)?;

            // Insert a dummy block number to set the canonical block
            let block_hash = random_b256();
            let block_number = 1u64;
            header_numbers.insert(block_hash.as_slice(), &block_number.to_le_bytes())?;

            let memory_db = new_from_exported_sled::<1024, 0>(&sled_db)?;

            assert!(memory_db.storage.contains_key::<Address>(&address.into()));

            let stored_storage = memory_db.storage.get::<Address>(&address.into()).unwrap();
            assert!(stored_storage.contains_key(&storage_key.into()));
            assert_eq!(
                stored_storage
                    .get(&storage_key.into())
                    .unwrap()
                    .get_latest(),
                Some(&storage_value)
            );

            Ok(())
        }

        #[test]
        fn test_insert_multiple_storage_entries() -> Result<(), Box<dyn std::error::Error>> {
            let sled_db = create_mock_sled_db();
            let plain_storage_state = sled_db.open_tree("PlainStorageState")?;
            let header_numbers = sled_db.open_tree("HeaderNumbers")?;

            let address = random_address();
            let storage_entries = vec![
                (random_b256(), U256::from(42)),
                (random_b256(), U256::from(100)),
                (random_b256(), U256::from(200)),
            ];

            let mut storage_map = HashMap::new();
            for (key, value) in &storage_entries {
                storage_map.insert(key.to_vec(), value.to_be_bytes::<32>().to_vec());
            }
            let serialized_map = bincode::serialize(&StorageMap(storage_map))?;
            plain_storage_state.insert(address.as_slice(), serialized_map)?;

            // Insert a dummy block number to set the canonical block
            let block_hash = random_b256();
            let block_number = 1u64;
            header_numbers.insert(block_hash.as_slice(), &block_number.to_le_bytes())?;

            let memory_db = new_from_exported_sled::<1024, 0>(&sled_db)?;

            assert!(memory_db.storage.contains_key::<Address>(&address.into()));
            let stored_storage = memory_db.storage.get::<Address>(&address.into()).unwrap();
            assert_eq!(stored_storage.len(), storage_entries.len());

            for (key, value) in storage_entries {
                assert!(stored_storage.contains_key(&key.into()));
                assert_eq!(
                    stored_storage.get(&key.into()).unwrap().get_latest(),
                    Some(&value)
                );
            }

            Ok(())
        }


        #[test]
        fn test_insert_multiple_storage_entries_with_load_from_exported_sled() -> Result<(), Box<dyn std::error::Error>> {
            let sled_db = create_mock_sled_db();
            let plain_storage_state = sled_db.open_tree("PlainStorageState")?;
            let header_numbers = sled_db.open_tree("HeaderNumbers")?;

            let address = random_address();
            let storage_entries = vec![
                (random_b256(), U256::from(42)),
                (random_b256(), U256::from(100)),
                (random_b256(), U256::from(200)),
            ];

            let mut storage_map = HashMap::new();
            for (key, value) in &storage_entries {
                storage_map.insert(key.to_vec(), value.to_be_bytes::<32>().to_vec());
            }
            let serialized_map = bincode::serialize(&StorageMap(storage_map))?;
            plain_storage_state.insert(address.as_slice(), serialized_map)?;

            // Insert a dummy block number to set the canonical block
            let block_hash = random_b256();
            let block_number = 1u64;
            header_numbers.insert(block_hash.as_slice(), &block_number.to_le_bytes())?;

            let mut memory_db: MemoryDb<5> = MemoryDb::default();
            memory_db.load_from_exported_sled(&sled_db)?;


            assert!(memory_db.storage.contains_key::<Address>(&address.into()));
            let stored_storage = memory_db.storage.get::<Address>(&address.into()).unwrap();
            assert_eq!(stored_storage.len(), storage_entries.len());

            for (key, value) in storage_entries {
                assert!(stored_storage.contains_key(&key.into()));
                assert_eq!(
                    stored_storage.get(&key.into()).unwrap().get_latest(),
                    Some(&value)
                );
            }

            Ok(())
        }

    }

    mod memory_db_db_ref_tests {
        use super::*;
        #[test]
        fn test_basic_ref() {
            let mut db = MemoryDb::<0>::default();
            let address = random_bytes().into();
            let code = Bytecode::LegacyRaw(Bytes::from(random_bytes::<64>()));
            let account_info = AccountInfo {
                nonce: 1,
                balance: U256::from(1000),
                code_hash: random_bytes(),
                code: Some(code.clone()),
            };

            let state_changes = HashMap::from([(
                address,
                Account {
                    info: account_info.clone(),
                    storage: HashMap::new(),
                    status: AccountStatus::Touched,
                },
            )]);
            db.commit_block(BlockChanges {
                state_changes,
                ..Default::default()
            });

            let result = db.basic_ref(address);
            assert_eq!(result.unwrap(), Some(account_info));
        }

        #[test]
        fn test_block_hash_ref() {
            let mut db = MemoryDb::<0>::default();
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
            let mut db = MemoryDb::<0>::default();
            let address = random_bytes().into();
            let slot = U256::from(1);
            let value = U256::from(100);

            let state_changes = HashMap::from([(
                address,
                Account {
                    info: AccountInfo {
                        nonce: 0,
                        balance: U256::ZERO,
                        code_hash: B256::ZERO,
                        code: Some(Bytecode::LegacyRaw(Bytes::from(random_bytes::<64>()))),
                    },
                    storage: HashMap::from([(
                        slot,
                        EvmStorageSlot {
                            present_value: value,
                            original_value: U256::ZERO,
                            is_cold: false,
                        },
                    )]),
                    status: AccountStatus::Touched,
                },
            )]);
            // Commit the storage data
            db.commit_block(BlockChanges {
                state_changes,
                ..Default::default()
            });

            let result = db.storage_ref(address, slot);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), value);

            let non_existent_slot = U256::from(2);
            let result = db.storage_ref(address, non_existent_slot);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), U256::ZERO);
        }

        //Should error if the code hash is not found
        #[test]
        fn test_code_by_hash_ref() {
            let mut db = MemoryDb::<0>::default();
            let code_hash = random_bytes();
            let code = Bytecode::LegacyRaw(Bytes::from(random_bytes::<96>()));

            let state_changes = HashMap::from([(
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
            )]);

            // Commit the code data
            db.commit_block(BlockChanges {
                state_changes,
                ..Default::default()
            });

            let result = db.code_by_hash_ref(code_hash);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), code);

            let non_existent_hash = random_bytes();
            let result = db.code_by_hash_ref(non_existent_hash);
            assert!(result.is_err());
        }
    }
    #[cfg(test)]
    mod memory_db_tests_commit_block {
        use super::*;
        use crate::primitives::{
            AccountInfo,
            Bytecode,
            EvmStorageSlot,
        };

        //TODO: improve
        #[test]
        fn test_commit_block() {
            let mut db = MemoryDb::<1>::default();
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

            db.commit_block(BlockChanges {
                block_num: 0,
                block_hash: random_bytes(),
                state_changes: changes.clone(),
            });

            let res = db.commit_block(BlockChanges {
                block_num: 1,
                block_hash: random_bytes(),
                state_changes: changes.clone(),
            });

            // Verify the changes
            assert_eq!(db.canonical_block_num, 1);
            assert_eq!(&db.canonical_block_hash, db.block_hashes.get(&1).unwrap());

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
            assert_eq!(&block_changes.block_hash, db.block_hashes.get(&1).unwrap());
            assert_eq!(block_changes.state_changes, changes);

            assert_eq!(res.block_changes, block_changes);
            assert_eq!(res.block_changes_to_remove, vec![0]);
            assert_eq!(
                res.code_hashes_to_insert,
                vec![(new_account_info.code_hash, new_code)]
            );

            println!("{:#?}", res.account_value_histories);
            println!("{:#?}", res.storage_value_histories);
            assert_eq!(res.account_value_histories.len(), 1);
            assert_eq!(res.storage_value_histories.len(), 1);
            assert_eq!(res.account_value_histories[0].value_history.len(), 2);
            assert_eq!(res.storage_value_histories[0].value_history.len(), 2);
        }
    }

    mod memory_db_tests_handle_reorg {
        use super::*;

        #[test]
        fn test_handle_reorg() {
            let mut db = MemoryDb::<5>::default();

            let block_changes_0 = BlockChanges {
                block_num: 0,
                block_hash: random_bytes(),
                state_changes: HashMap::from([(
                    random_bytes().into(),
                    Account {
                        info: AccountInfo {
                            nonce: 0,
                            balance: U256::from(1000),
                            code_hash: B256::ZERO,
                            code: Some(Bytecode::LegacyRaw(Bytes::from(random_bytes::<64>()))),
                        },
                        storage: HashMap::from([(
                            U256::from(1),
                            EvmStorageSlot {
                                present_value: U256::from(100),
                                original_value: U256::ZERO,
                                is_cold: false,
                            },
                        )]),
                        status: AccountStatus::Touched,
                    },
                )]),
            };
            db.commit_block(block_changes_0.clone());

            let block_changes_1 = BlockChanges {
                block_num: 1,
                block_hash: random_bytes(),
                state_changes: HashMap::from([(
                    random_bytes().into(),
                    Account {
                        info: AccountInfo {
                            nonce: 0,
                            balance: U256::ZERO,
                            code_hash: random_bytes(),
                            code: Some(Bytecode::LegacyRaw(Bytes::from(random_bytes::<64>()))),
                        },
                        storage: HashMap::from([(
                            U256::from(1),
                            EvmStorageSlot {
                                present_value: U256::from(200),
                                original_value: U256::ZERO,
                                is_cold: false,
                            },
                        )]),
                        status: AccountStatus::Touched,
                    },
                )]),
            };
            db.commit_block(block_changes_1.clone());

            let result = db.handle_reorg(block_changes_0.block_hash).unwrap();

            assert_eq!(result.block_numbers_to_remove, vec![1]);

            assert_eq!(result.account_value_histories.len(), 1);
            assert!(result.account_value_histories[0].value_history.is_empty());

            assert_eq!(result.storage_value_histories.len(), 1);
            assert!(result.storage_value_histories[0].value_history.is_empty());

            assert_eq!(
                &result.account_value_histories[0].address,
                block_changes_1.state_changes.keys().next().unwrap()
            );

            assert_eq!(db.canonical_block_num, block_changes_0.block_num);
            assert_eq!(db.canonical_block_hash, block_changes_0.block_hash);
            assert_eq!(db.block_changes.len(), 1);

            assert_eq!(db.basic.len(), 1);
            assert_eq!(db.storage.len(), 1);
            assert_eq!(db.storage.iter().next().unwrap().1.len(), 1);
            assert_eq!(
                db.storage
                    .iter()
                    .next()
                    .unwrap()
                    .1
                    .values()
                    .next()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(db.block_hashes.len(), 1);
            assert_eq!(db.block_hashes.get(&0), Some(&block_changes_0.block_hash));
            assert_eq!(db.code_by_hash.len(), 2);

            let account_change = block_changes_0.state_changes.iter().next().unwrap().1;

            assert_eq!(
                db.basic
                    .get(block_changes_0.state_changes.keys().next().unwrap())
                    .unwrap()
                    .get_latest()
                    .cloned()
                    .unwrap(),
                account_change.info
            );
            assert_eq!(
                db.storage
                    .iter()
                    .next()
                    .unwrap()
                    .1
                    .iter()
                    .next()
                    .unwrap()
                    .1
                    .get_latest()
                    .cloned()
                    .unwrap(),
                account_change
                    .storage
                    .iter()
                    .next()
                    .unwrap()
                    .1
                    .present_value
            );

            assert_eq!(db.block_changes[0], block_changes_0);
        }
    }

    //TODO: Test case where reorg not found
    //TODO: Add integration test to verify revm handles block_hash access based on evm version
    //
    //TODO: Test the canonicalize function with self destruct in uncle block
}
