#![cfg(any(test, feature = "test"))]

//! # Fork test
//!
//! The following EVM code contains a simple contract that increments a number when called.
//! A transaction is called on the contract that increments the number stored.
//!
//! The contract has an assertion that prevents the number inside of the contract from changing.
//! After every transaction, the assertion creates a fork before and after transaction execution,
//! reads and checks the storage slot containing the number.
//! If the number has changed, the assertion fails.

use assertion_executor::{
    db::SharedDB,
    primitives::{
        Account,
        AccountStatus,
        BlockChanges,
    },
    AssertionExecutor,
};

// Re-export test util items
pub use assertion_executor::test_utils::{
    counter_acct_info,
    counter_call,
    COUNTER_ADDRESS as FORK_ADDRESS,
};

/// Deploys the fork test contract.
pub fn deploy_fork_test(
    _executor: &mut AssertionExecutor<SharedDB<5>>,
    block_changes: &mut BlockChanges,
) {
    // Fork test contract to block_changes
    block_changes.state_changes.insert(
        FORK_ADDRESS,
        Account {
            info: counter_acct_info(),
            status: AccountStatus::Touched | AccountStatus::Created,
            ..Default::default()
        },
    );
}
