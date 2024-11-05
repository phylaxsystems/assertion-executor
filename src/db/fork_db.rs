use crate::{
    db::{
        DatabaseCommit,
        DatabaseRef,
    },
    primitives::{
        AccountInfo,
        Address,
        Bytecode,
        EvmState,
        B256,
        U256,
    },
};
use std::collections::HashMap;

/// Maps storage slots to their values.
/// Also contains a flag to indicate if the account is self destructed.
/// If the account is self destructed, it won't read from the inner database.
#[derive(Debug, Clone, Default)]
pub(super) struct ForkStorageMap {
    pub map: HashMap<U256, U256>,
    self_destructed: bool,
}

/// Contains mutations on top of an existing database.
#[derive(Debug, Clone)]
pub struct ForkDb<ExtDb> {
    /// Maps addresses to storage slots and their history indexed by block.
    pub(super) storage: HashMap<Address, ForkStorageMap>,
    /// Maps addresses to their account info and indexes it by block.
    pub(super) basic: HashMap<Address, AccountInfo>,
    /// Maps bytecode hashes to bytecode.
    pub(super) code_by_hash: HashMap<B256, Bytecode>,
    /// Inner database.
    pub(super) inner_db: ExtDb,
}

impl<ExtDb: DatabaseRef> DatabaseRef for ForkDb<ExtDb> {
    type Error = <ExtDb as DatabaseRef>::Error;
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        match self.basic.get(&address) {
            Some(b) => Ok(Some(b.clone())),
            None => Ok(self.inner_db.basic_ref(address)?),
        }
    }
    fn storage_ref(&self, address: Address, slot: U256) -> Result<U256, Self::Error> {
        match self.storage.get(&address) {
            Some(s) => {
                // If the account is self destructed, do not read from inner db.
                if s.self_destructed {
                    return Ok(*s.map.get(&slot).unwrap_or(&U256::ZERO));
                }

                match s.map.get(&slot) {
                    Some(v) => Ok(*v),
                    None => Ok(self.inner_db.storage_ref(address, slot)?),
                }
            }
            None => Ok(self.inner_db.storage_ref(address, slot)?),
        }
    }
    fn code_by_hash_ref(&self, hash: B256) -> Result<Bytecode, Self::Error> {
        match self.code_by_hash.get(&hash) {
            Some(code) => Ok(code.clone()),
            None => Ok(self.inner_db.code_by_hash_ref(hash)?),
        }
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.inner_db.block_hash_ref(number)
    }
}

impl<ExtDb> DatabaseCommit for ForkDb<ExtDb> {
    fn commit(&mut self, changes: EvmState) {
        for (address, account) in changes {
            if !account.is_touched() {
                continue;
            }
            if account.is_selfdestructed() {
                self.basic.insert(address, account.info.clone());

                let fork_storage_map = self.storage.entry(address).or_default();

                // Mark the account as self destructed.
                // This will prevent reading from the inner database.
                fork_storage_map.self_destructed = true;
                fork_storage_map.map.clear();

                continue;
            }

            if account.info.code.is_some() {
                self.code_by_hash
                    .insert(account.info.code_hash, account.info.code.clone().unwrap());
            }

            self.basic.insert(address, account.info.clone());
            match self.storage.get_mut(&address) {
                Some(s) => {
                    s.map.extend(
                        account
                            .storage
                            .into_iter()
                            .map(|(k, v)| (k, v.present_value())),
                    );
                }
                None => {
                    self.storage.insert(
                        address,
                        ForkStorageMap {
                            map: account
                                .storage
                                .into_iter()
                                .map(|(k, v)| (k, v.present_value()))
                                .collect(),
                            self_destructed: false,
                        },
                    );
                }
            }
        }
    }
}

impl<ExtDb> ForkDb<ExtDb> {
    /// Creates a new ForkDb.
    pub fn new(inner_db: ExtDb) -> Self {
        Self {
            storage: Default::default(),
            basic: Default::default(),
            code_by_hash: Default::default(),
            inner_db,
        }
    }

    /// Replace the inner database with a new one.
    /// Returns the previous inner database.
    pub fn replace_inner_db(&mut self, new_db: ExtDb) -> ExtDb {
        std::mem::replace(&mut self.inner_db, new_db)
    }

    /// Inserts an account info into the fork db.
    /// This will overwrite any existing account info.
    pub fn insert_account_info(&mut self, address: Address, account_info: AccountInfo) {
        self.basic.insert(address, account_info);
    }
}

#[cfg(test)]
mod fork_db_tests {
    use super::*;

    use crate::{
        db::{
            DatabaseRef,
            SharedDB,
        },
        primitives::{
            uint,
            Account,
            AccountStatus,
            BlockChanges,
            EvmStorageSlot,
            U256,
        },
        test_utils::random_bytes,
    };

    use revm::primitives::{
        keccak256,
        KECCAK_EMPTY,
    };
    use std::collections::HashMap;

    #[test]
    fn test_basic() {
        let mut shared_db = SharedDB::<0>::new_test();

        let mut block_changes = BlockChanges::default();
        let mut evm_state = EvmState::default();

        evm_state.insert(
            Address::ZERO,
            Account {
                info: AccountInfo {
                    nonce: 0,
                    balance: uint!(1000_U256),
                    ..Default::default()
                },
                storage: HashMap::new(),
                status: AccountStatus::Touched,
            },
        );

        block_changes.state_changes = evm_state.clone();

        let _ = shared_db.commit_block(block_changes).unwrap();

        let mut fork_db = shared_db.fork();

        assert_eq!(
            shared_db.basic_ref(Address::ZERO).unwrap().unwrap().balance,
            uint!(1000_U256)
        );

        assert_eq!(
            fork_db.basic_ref(Address::ZERO).unwrap().unwrap().balance,
            uint!(1000_U256)
        );

        evm_state.get_mut(&Address::ZERO).unwrap().info.balance = uint!(2000_U256);

        fork_db.commit(evm_state);

        assert_eq!(
            fork_db.basic_ref(Address::ZERO).unwrap().unwrap().balance,
            uint!(2000_U256)
        );
        assert_eq!(
            shared_db.basic_ref(Address::ZERO).unwrap().unwrap().balance,
            uint!(1000_U256)
        );
    }

    #[test]
    fn test_storage() {
        let mut shared_db = SharedDB::<0>::new_test();

        let mut block_changes = BlockChanges::default();
        let mut evm_state = EvmState::default();

        let mut storage = HashMap::new();

        let mut evm_storage_slot = EvmStorageSlot {
            original_value: uint!(0_U256),
            present_value: uint!(1_U256),
            is_cold: false,
        };

        storage.insert(uint!(0_U256), evm_storage_slot.clone());

        evm_state.insert(
            Address::ZERO,
            Account {
                info: AccountInfo::default(),
                storage: storage.clone(),
                status: AccountStatus::Touched,
            },
        );

        block_changes.state_changes = evm_state.clone();

        let _ = shared_db.commit_block(block_changes).unwrap();

        let mut fork_db = shared_db.fork();

        assert_eq!(
            shared_db.storage_ref(Address::ZERO, uint!(0_U256)).unwrap(),
            uint!(1_U256)
        );

        assert_eq!(
            fork_db.storage_ref(Address::ZERO, uint!(0_U256)).unwrap(),
            uint!(1_U256)
        );

        evm_storage_slot.original_value = uint!(1_U256);
        evm_storage_slot.present_value = uint!(2_U256);

        evm_state
            .get_mut(&Address::ZERO)
            .unwrap()
            .storage
            .insert(uint!(0_U256), evm_storage_slot);

        fork_db.commit(evm_state);

        assert_eq!(
            shared_db.storage_ref(Address::ZERO, uint!(0_U256)).unwrap(),
            uint!(1_U256)
        );

        assert_eq!(
            fork_db.storage_ref(Address::ZERO, uint!(0_U256)).unwrap(),
            uint!(2_U256)
        );
    }

    #[test]
    fn test_code_by_hash() {
        let mut shared_db = SharedDB::<0>::new_test();
        let mut evm_state = EvmState::default();
        let mut fork_db = shared_db.fork();

        assert_eq!(
            shared_db.code_by_hash_ref(KECCAK_EMPTY),
            Err(crate::db::NotFoundError)
        );
        assert_eq!(
            fork_db.code_by_hash_ref(KECCAK_EMPTY),
            Err(crate::db::NotFoundError)
        );

        let code_bytes = random_bytes::<32>();
        let code_hash = keccak256(&code_bytes);
        let bytecode = Bytecode::new_raw(code_bytes.into());

        evm_state.insert(
            Address::ZERO,
            Account {
                info: AccountInfo {
                    code_hash: code_hash.clone(),
                    code: Some(bytecode.clone()),
                    ..Default::default()
                },
                storage: HashMap::new(),
                status: AccountStatus::Touched,
            },
        );
        let _ = shared_db.commit_block(BlockChanges {
            state_changes: evm_state,
            ..Default::default()
        });

        assert_eq!(
            shared_db.code_by_hash_ref(code_hash.clone()).unwrap(),
            bytecode
        );

        assert_eq!(
            fork_db.code_by_hash_ref(code_hash.clone()).unwrap(),
            bytecode
        );

        let mut evm_state = EvmState::default();

        let code_bytes = random_bytes::<64>();
        let code_hash = keccak256(&code_bytes);
        let bytecode = Bytecode::new_raw(code_bytes.into());

        evm_state.insert(
            Address::ZERO,
            Account {
                info: AccountInfo {
                    code_hash: code_hash.clone(),
                    code: Some(bytecode.clone()),
                    ..Default::default()
                },
                storage: HashMap::new(),
                status: AccountStatus::Touched,
            },
        );

        fork_db.commit(evm_state);

        assert_eq!(
            shared_db.code_by_hash_ref(code_hash.clone()),
            Err(crate::db::NotFoundError)
        );

        assert_eq!(
            fork_db.code_by_hash_ref(code_hash.clone()).unwrap(),
            bytecode
        );
    }

    #[test]
    fn test_block_hash() {
        let mut shared_db = SharedDB::<0>::new_test();

        let mut fork_db = shared_db.fork();
        assert_eq!(shared_db.block_hash_ref(0), Err(crate::db::NotFoundError));

        let mut block_changes = BlockChanges {
            block_num: 0,
            block_hash: KECCAK_EMPTY,
            ..Default::default()
        };

        let _ = shared_db.commit_block(block_changes).unwrap();
        assert_eq!(shared_db.block_hash_ref(0), Ok(KECCAK_EMPTY));
    }

    #[test]
    fn test_commit_self_destruct() {
        let mut shared_db = SharedDB::<0>::new_test();

        let mut block_changes = BlockChanges::default();
        let mut evm_state = EvmState::default();

        let mut storage = HashMap::new();

        let mut evm_storage_slot = EvmStorageSlot {
            original_value: uint!(0_U256),
            present_value: uint!(1_U256),
            is_cold: false,
        };

        storage.insert(uint!(0_U256), evm_storage_slot.clone());

        evm_state.insert(
            Address::ZERO,
            Account {
                info: AccountInfo::default(),
                storage: storage.clone(),
                status: AccountStatus::Touched,
            },
        );
        let _ = shared_db.commit_block(BlockChanges {
            state_changes: evm_state,
            ..Default::default()
        });
        let mut fork_db = shared_db.fork();
        assert_eq!(
            shared_db.storage_ref(Address::ZERO, uint!(0_U256)).unwrap(),
            evm_storage_slot.present_value()
        );
        assert_eq!(
            fork_db.storage_ref(Address::ZERO, uint!(0_U256)).unwrap(),
            evm_storage_slot.present_value()
        );

        let mut evm_state = EvmState::default();
        evm_state.insert(
            Address::ZERO,
            Account {
                info: AccountInfo::default(),
                storage: HashMap::new(),
                status: AccountStatus::SelfDestructed | AccountStatus::Touched,
            },
        );

        fork_db.commit(evm_state);

        assert_eq!(
            shared_db.storage_ref(Address::ZERO, uint!(0_U256)).unwrap(),
            evm_storage_slot.present_value()
        );

        assert_eq!(
            fork_db.storage_ref(Address::ZERO, uint!(0_U256)).unwrap(),
            U256::ZERO
        );
    }
}
