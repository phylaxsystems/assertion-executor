use crate::error::ExecutorError;
pub use revm::primitives::{
    address,
    Account,
    AccountInfo,
    Address,
    BlockEnv,
    Bytecode,
    EVMError,
    ExecutionResult,
    FixedBytes,
    TxEnv,
    TxKind,
    B256,
    U256,
};

use std::collections::{
    BTreeMap,
    HashMap,
};

#[derive(Default, Debug, Clone, PartialEq)]
pub struct AssertionContract {
    pub fn_selectors: Vec<FixedBytes<4>>,
    pub code: Bytecode,
    pub code_hash: B256,
}

pub type BuiltBlock = (Vec<TransactionBundle>, BlockEnv);

/// Contains raw TxEnv as well as the received RPC call.
#[derive(Debug, Clone, Default)]
pub struct TransactionBundle {
    pub decoded: TxEnv,
    pub raw: String,
}

pub struct AssertionId {
    pub fn_selector: FixedBytes<4>,
    pub code_hash: B256,
}

pub struct AssertionResult {
    pub id: AssertionId,
    pub result: Result<ExecutionResult, ExecutorError>,
}

pub type StateChanges = HashMap<Address, Account>;

#[derive(Debug, Clone)]
pub struct BlockChanges {
    pub block_num: u64,
    pub block_hash: B256,
    pub state_changes: StateChanges,
}

impl AssertionResult {
    pub fn is_success(&self) -> bool {
        self.result.is_ok() && self.result.as_ref().unwrap().is_success()
    }
}

/// A history of values at different block numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValueHistory<T> {
    value_history: BTreeMap<u64, T>,
}

impl<T> ValueHistory<T> {
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
