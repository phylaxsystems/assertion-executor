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
pub struct MemoryDb {
    /// Maps addresses to storage slots and their history indexed by block.
    pub(super) storage: EvmStorage,
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

        for table_name in reth_tables {
            match *table_name {
                // <Hash, Blocknumber>
                // Loads `block_hashes` as well as the `canonical_block_hash` and `canonical_block_num`
                // NOTE: this table must be deserialized first
                "HeaderNumbers" => {
                    let result = Self::load_header_numbers(sled_db, &mut memory_db, table_name)?;
                    canonical_block_hash = result.0;
                    canonical_block_num = result.1;
                }
                // <Code hash, Bytecode>
                // Loads `code_by_hash` and supplements `basic`
                "Bytecodes" => Self::load_bytecodes(sled_db, &mut memory_db, table_name)?,
                // <Address, Account>
                // Loads `basic`
                // NEEDS `Bytecodes` to be completed before this
                "PlainAccountState" => {
                    Self::load_plain_account_state(
                        sled_db,
                        &mut memory_db,
                        table_name,
                        canonical_block_num,
                    )?
                }
                // <Address, Hashmap<slot, value>>
                // Loads `storage`
                // NEEDS `HeaderNumbers` to be completed before this.
                // TODO: double check this?
                "PlainStorageState" => {
                    Self::load_plain_storage_state(
                        sled_db,
                        &mut memory_db,
                        table_name,
                        canonical_block_num,
                    )?
                }
                _ => return Err(MemoryDbError::LoadFail),
            }
        }

        memory_db.canonical_block_hash = canonical_block_hash;
        memory_db.canonical_block_num = canonical_block_num;

        Ok(memory_db)
    }

    fn load_header_numbers<const LEAF_FANOUT: usize>(
        sled_db: &SledDb<LEAF_FANOUT>,
        memory_db: &mut MemoryDb,
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

                memory_db.block_hashes.insert(value, key);
            }
        }

        Ok((canonical_block_hash, canonical_block_num))
    }

    fn load_bytecodes<const LEAF_FANOUT: usize>(
        sled_db: &SledDb<LEAF_FANOUT>,
        memory_db: &mut MemoryDb,
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
                memory_db.code_by_hash.insert(key, bytecode);
            }
        }

        Ok(())
    }

    fn load_plain_account_state<const LEAF_FANOUT: usize>(
        sled_db: &SledDb<LEAF_FANOUT>,
        memory_db: &mut MemoryDb,
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
                let (code_hash, code) = Self::get_code_info(memory_db, &value);

                let account_info: AccountInfo = AccountInfo {
                    nonce: value.nonce,
                    balance: value.balance,
                    code_hash,
                    code,
                };

                let mut history = ValueHistory::new();
                history.insert(canonical_block_num, account_info);
                memory_db.basic.insert(key, history);
            }
        }

        Ok(())
    }

    fn get_code_info(
        memory_db: &MemoryDb,
        value: &RethAccount,
    ) -> (FixedBytes<32>, Option<Bytecode>) {
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
            if let Some(bytecode) = memory_db.code_by_hash.get(&bytecode_hash) {
                code = Some(bytecode.clone());
            }
        }

        (code_hash, code)
    }

    fn load_plain_storage_state<const LEAF_FANOUT: usize>(
        sled_db: &SledDb<LEAF_FANOUT>,
        memory_db: &mut MemoryDb,
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
                let address_storage = memory_db.storage.entry(key).or_default();

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
    /// `mending::transform_reth_tables`.
    pub fn new_from_exported_sled<const LEAF_FANOUT: usize>(
        sled_db: &SledDb<LEAF_FANOUT>,
    ) -> Result<Self, MemoryDbError> {
        let memory_db = MemoryDb::default();
        Self::load_tables_into_db::<LEAF_FANOUT>(sled_db, memory_db)
    }

    /// Opens a `sled` Db that has had exported and transformed data from `reth` MDBX with
    /// `mending::transform_reth_tables` and loads it into itself.
    pub fn load_into_self(&mut self, sled_db: &SledDb) -> Result<(), MemoryDbError> {
        let memory_db = MemoryDb::default();
        let loaded_db = Self::load_tables_into_db(sled_db, memory_db)?;
        *self = loaded_db;
        Ok(())
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

// Replace with commit_block in trait PhDB trait
// Will need to prune history and update sled
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
    use std::collections::HashMap;

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

        let memory_db = MemoryDb::new_from_exported_sled(&sled_db)?;

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

        let memory_db = MemoryDb::new_from_exported_sled(&sled_db)?;

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

        let memory_db = MemoryDb::new_from_exported_sled(&sled_db)?;

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

        let memory_db = MemoryDb::new_from_exported_sled(&sled_db)?;

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

        let memory_db = MemoryDb::new_from_exported_sled(&sled_db)?;

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
//TODO: Add integration test to verify revm handles block_hash access based on evm version
