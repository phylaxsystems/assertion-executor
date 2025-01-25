use crate::db::fs::serde::StorageSlotKey;
pub use revm::{
    db::AccountState,
    primitives::{
        address,
        bytes,
        fixed_bytes,
        hex,
        keccak256,
        result::ResultAndState,
        uint,
        Account,
        AccountInfo,
        AccountStatus,
        Address,
        BlockEnv,
        Bytecode,
        Bytes,
        EVMError,
        EvmState,
        EvmStorage,
        EvmStorageSlot,
        ExecutionResult as EvmExecutionResult,
        FixedBytes,
        Output,
        SpecId,
        TxEnv,
        TxKind,
        B256,
        U256,
    },
    JournaledState,
};

use std::collections::{
    BTreeMap,
    HashSet,
};

#[derive(Default, Debug, Clone, PartialEq, Hash, Eq)]
pub struct AssertionContract {
    pub fn_selectors: Vec<FixedBytes<4>>,
    pub code: Bytecode,
    pub code_hash: B256,
}

#[derive(Debug)]
pub struct AssertionId {
    pub fn_selector: FixedBytes<4>,
    pub code_hash: B256,
}

#[derive(Debug)]
pub struct AssertionResult {
    pub id: AssertionId,
    pub result: AssertionExecutionResult,
}

#[derive(Debug)]
pub enum AssertionExecutionResult {
    AssertionContractDeployFailure(EvmExecutionResult),
    AssertionExecutionResult(EvmExecutionResult),
}

impl AssertionResult {
    pub fn is_success(&self) -> bool {
        if let AssertionExecutionResult::AssertionExecutionResult(result) = &self.result {
            result.is_success()
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockChanges {
    pub block_num: u64,
    pub block_hash: B256,
    pub state_changes: EvmState,
}

impl BlockChanges {
    /// Create a new BlockChanges instance, with empty state changes.
    pub fn new(block_num: u64, block_hash: B256) -> Self {
        Self {
            block_num,
            block_hash,
            state_changes: Default::default(),
        }
    }
    /// Merge `Vec<HashMap<Address, Account>>` into block changes.
    pub fn merge_state(&mut self, evm_state: EvmState) {
        // Process states in order so later states override earlier ones for overlapping values
        for (key, value) in evm_state {
            if let Some(existing_account) = self.state_changes.get_mut(&key) {
                // Update account info and status from latest
                existing_account.info = value.info;
                // Update account info and status from latest
                existing_account.status = value.status;
                // Merge storage - keep old slots but override with new values when they exist
                existing_account.storage.extend(value.storage);
            } else {
                self.state_changes.insert(key, value);
            }
        }
    }
}

///code_by_hash mapping is currently append only.
///Code hashes can only be removed if all accounts with that code hash are self destructed, or
///reorged out of creation.
///
///Not handling this could result in minor storage bloat;
#[derive(Debug, Clone, Default)]
pub struct TouchedKeys {
    pub basic: HashSet<Address>,
    pub storage: HashSet<StorageSlotKey>,
}
impl TouchedKeys {
    pub fn extend(&mut self, other: TouchedKeys) {
        self.basic.extend(other.basic);
        self.storage.extend(other.storage);
    }
}

impl BlockChanges {
    pub fn touched_keys(&self) -> TouchedKeys {
        self.state_changes.iter().fold(
            TouchedKeys::default(),
            |mut touched_keys, (address, account)| {
                touched_keys.basic.insert(*address);

                account.storage.keys().for_each(|slot| {
                    touched_keys.storage.insert(StorageSlotKey {
                        address: *address,
                        slot: *slot,
                    });
                });
                touched_keys
            },
        )
    }
}

/// A history of values at different block numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValueHistory<T> {
    pub value_history: BTreeMap<u64, T>,
}

impl<T> ValueHistory<T> {
    /// Returns the number of values in the ValueHistory.
    pub fn len(&self) -> usize {
        self.value_history.len()
    }

    /// Returns true if the ValueHistory is empty.
    pub fn is_empty(&self) -> bool {
        self.value_history.is_empty()
    }

    /// Get the latest value.
    /// Returns None if the ValueHistory is empty.
    pub fn get_latest(&self) -> Option<&T> {
        self.value_history.last_key_value().map(|(_, v)| v)
    }

    /// Insert a value at a block number.
    /// Returns the previous value at that block number, if it exists.
    pub fn insert(&mut self, block_num: u64, value: T) -> Option<T> {
        self.value_history.insert(block_num, value)
    }

    /// Prune the ValueHistory from the beginning of the tree to the block number.
    /// The block number is inclusive, meaning the value at that block number will be pruned, if it
    /// exists.
    /// Returns the pruned values.
    pub fn prune_to(&mut self, block_num: u64) -> ValueHistory<T> {
        let new_history = self.value_history.split_off(&(block_num + 1));
        ValueHistory {
            value_history: std::mem::replace(&mut self.value_history, new_history),
        }
    }

    /// Prune the ValueHistory from the block number, to the end of the tree.
    /// The block number is not inclusive, meaning the value at that block number will not be
    /// pruned.
    /// Returns the pruned values.
    pub fn prune_from(&mut self, block_num: u64) -> ValueHistory<T> {
        ValueHistory {
            value_history: self.value_history.split_off(&(block_num + 1)),
        }
    }

    pub fn new() -> Self {
        Self {
            value_history: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_insert_and_get_latest() {
        let mut history = ValueHistory::default();

        // Initially, the history is empty
        assert_eq!(history.get_latest(), None);

        // Insert a value and check if it's the latest
        history.insert(1, "value1");
        assert_eq!(history.get_latest(), Some(&"value1"));

        // Insert another value at a higher block number and check if it's the latest
        history.insert(2, "value2");
        assert_eq!(history.get_latest(), Some(&"value2"));

        // Insert a value at a lower block number (should not change the latest value)
        history.insert(0, "value0");
        assert_eq!(history.get_latest(), Some(&"value2"));
    }

    #[test]
    fn test_insert_returns_previous_value() {
        let mut history = ValueHistory::default();

        // Insert a value at block number 1
        assert_eq!(history.insert(1, "value1"), None);

        // Insert another value at block number 1 (should return the previous value)
        assert_eq!(history.insert(1, "value1_updated"), Some("value1"));
    }

    #[test]
    fn test_prune_to() {
        let mut history = ValueHistory::default();
        history.insert(1, "value1");
        history.insert(2, "value2");
        let expected_pruned = history.clone();

        history.insert(3, "value3");

        // Prune all entries up to block number 2 (inclusive)
        let pruned = history.prune_to(2);

        // Check the pruned values
        assert_eq!(pruned, expected_pruned, "Pruned values are incorrect");

        // Check if history only contains value for block number 3
        assert_eq!(
            history.get_latest(),
            Some(&"value3"),
            "Latest value is incorrect"
        );
        let mut expected_remaining = BTreeMap::new();
        expected_remaining.insert(3, "value3");
        assert_eq!(
            history.value_history, expected_remaining,
            "Remaining values are incorrect"
        );
    }

    #[test]
    fn test_prune_from() {
        let mut history = ValueHistory::default();
        history.insert(1, "value1");
        history.insert(2, "value2");
        history.insert(3, "value3");

        // Prune all entries starting from block number 2 (exclusive)
        let pruned = history.prune_from(2);

        // Check the pruned values
        let mut expected_pruned = ValueHistory::default();
        expected_pruned.insert(3, "value3");

        assert_eq!(pruned, expected_pruned, "Pruned values are incorrect");

        // After pruning, values up to block number 2 should remain
        let mut expected_remaining = ValueHistory::default();
        expected_remaining.insert(1, "value1");
        expected_remaining.insert(2, "value2");

        assert_eq!(
            history, expected_remaining,
            "Remaining values are incorrect"
        );

        // The latest value should still be block 2
        assert_eq!(
            history.get_latest(),
            Some(&"value2"),
            "Latest value is incorrect"
        );
    }

    #[test]
    fn test_prune_to_with_empty_map() {
        let mut history: ValueHistory<String> = ValueHistory::default();

        // Prune an empty history
        let pruned = history.prune_to(10);

        // The pruned values should be empty
        assert!(pruned.value_history.is_empty());
    }

    #[test]
    fn test_prune_from_with_empty_map() {
        let mut history: ValueHistory<String> = ValueHistory::default();

        // Prune an empty history
        let pruned = history.prune_from(10);

        // The pruned values should be empty
        assert!(pruned.value_history.is_empty());
    }
}
#[cfg(test)]
mod test_merge_state {
    use super::*;

    use std::str::FromStr;

    use std::collections::HashMap;

    // Helper functions remain the same
    fn create_test_account(balance: u64, nonce: u64, code: Vec<u8>) -> Account {
        Account {
            info: AccountInfo {
                balance: U256::from(balance),
                nonce,
                code_hash: FixedBytes::<32>::default(),
                code: Some(Bytecode::LegacyRaw(code.into())),
            },
            status: AccountStatus::default(),
            storage: HashMap::from_iter([]),
        }
    }

    fn create_storage_slot(value: u64) -> EvmStorageSlot {
        EvmStorageSlot::new(U256::from(value))
    }

    fn addr(s: &str) -> Address {
        Address::from_str(s).unwrap()
    }

    #[test]
    fn test_merge_storage_changes() {
        let mut state0 = HashMap::from_iter([]);
        let mut state1 = HashMap::from_iter([]);
        let mut state2 = HashMap::from_iter([]);

        let addr = addr("0x1000000000000000000000000000000000000000");

        // Initial state
        let mut account0 = create_test_account(100, 1, vec![0x60]);
        account0
            .storage
            .insert(U256::from(1), create_storage_slot(10));
        account0
            .storage
            .insert(U256::from(2), create_storage_slot(20));
        state0.insert(addr, account0);

        // State update 1
        let mut account1 = create_test_account(200, 2, vec![0x60]);
        account1
            .storage
            .insert(U256::from(2), create_storage_slot(25)); // Modify existing slot
        account1
            .storage
            .insert(U256::from(3), create_storage_slot(30)); // Add new slot
        state1.insert(addr, account1);

        // State update 2 (latest)
        let mut account2 = create_test_account(300, 3, vec![0x60]);
        account2
            .storage
            .insert(U256::from(3), create_storage_slot(35)); // Modify slot from state1
        account2
            .storage
            .insert(U256::from(4), create_storage_slot(40)); // Add new slot
        state2.insert(addr, account2);

        let block_changes = BlockChanges {
            state_changes: state0,
            ..Default::default()
        };

        // States pushed in order [state0, state1, state2]
        // After merging, we expect:
        // - Account info from state2 (balance 300, nonce 3)
        // - Combined storage with latest values taking precedence:
        //   - slot 1 = 10 (from state0, unchanged)
        //   - slot 2 = 25 (from state1, overrode state0)
        //   - slot 3 = 35 (from state2, overrode state1)
        //   - slot 4 = 40 (from state2)
        let mut merged = block_changes.clone();
        merged.merge_state(state1);
        merged.merge_state(state2);

        let merged_account = merged.state_changes.get(&addr).unwrap();

        // Verify account info is from latest state
        assert_eq!(
            merged_account.info.balance,
            U256::from(300),
            "Should have latest balance"
        );
        assert_eq!(merged_account.info.nonce, 3, "Should have latest nonce");

        // Verify storage has combined values with latest taking precedence
        assert_eq!(
            merged_account
                .storage
                .get(&U256::from(1))
                .expect("Storage slot 1 should exist")
                .present_value(),
            U256::from(10),
            "Should keep original value from state0"
        );

        assert_eq!(
            merged_account
                .storage
                .get(&U256::from(2))
                .expect("Storage slot 2 should exist")
                .present_value(),
            U256::from(25),
            "Should have state1's value for slot 2"
        );

        assert_eq!(
            merged_account
                .storage
                .get(&U256::from(3))
                .expect("Storage slot 3 should exist")
                .present_value(),
            U256::from(35),
            "Should have state2's value for slot 3"
        );

        assert_eq!(
            merged_account
                .storage
                .get(&U256::from(4))
                .expect("Storage slot 4 should exist")
                .present_value(),
            U256::from(40),
            "Should have state2's value for slot 4"
        );
    }

    #[test]
    fn test_merge_status_changes() {
        let mut state0 = HashMap::from_iter([]);
        let mut state1 = HashMap::from_iter([]);

        let addr = addr("0x1000000000000000000000000000000000000000");

        // Initial state
        let mut account0 = create_test_account(100, 1, vec![]);
        account0.status = AccountStatus::default();
        state0.insert(addr, account0);

        // Updated state (latest)
        let mut account1 = create_test_account(200, 2, vec![]);
        account1.status = AccountStatus::default();
        state1.insert(addr, account1);

        let block_changes = BlockChanges {
            state_changes: state0,
            ..Default::default()
        };

        let mut merged = block_changes.clone();
        merged.merge_state(state1);

        let merged_account = merged.state_changes.get(&addr).unwrap();

        // Verify account info is from latest state
        assert_eq!(
            merged_account.info.balance,
            U256::from(200),
            "Should have latest balance"
        );
        assert_eq!(merged_account.info.nonce, 2, "Should have latest nonce");
    }

    #[test]
    fn test_merge_new_accounts() {
        let mut state0 = HashMap::from_iter([]);
        let mut state1 = HashMap::from_iter([]);

        let addr1 = addr("0x1000000000000000000000000000000000000000");
        let addr2 = addr("0x2000000000000000000000000000000000000000");

        // First state has account1
        let account1_initial = create_test_account(100, 1, vec![]);
        state0.insert(addr1, account1_initial);

        // Second state updates account1 and adds account2
        let account1_updated = create_test_account(200, 2, vec![]);
        let account2 = create_test_account(300, 1, vec![]);

        state1.insert(addr1, account1_updated);
        state1.insert(addr2, account2);

        let block_changes = BlockChanges {
            state_changes: state0,
            ..Default::default()
        };

        let mut merged = block_changes.clone();
        merged.merge_state(state1);

        // Verify account1 has latest state
        let merged_account1 = merged.state_changes.get(&addr1).unwrap();
        assert_eq!(
            merged_account1.info.balance,
            U256::from(200),
            "Account1 should have latest balance"
        );
        assert_eq!(
            merged_account1.info.nonce, 2,
            "Account1 should have latest nonce"
        );

        // Verify account2 exists with its state
        let merged_account2 = merged.state_changes.get(&addr2).unwrap();
        assert_eq!(
            merged_account2.info.balance,
            U256::from(300),
            "Account2 should be present with its balance"
        );
        assert_eq!(
            merged_account2.info.nonce, 1,
            "Account2 should be present with its nonce"
        );
    }
}
