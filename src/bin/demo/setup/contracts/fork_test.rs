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
        BlockChanges,
        BlockEnv,
        FixedBytes,
    },
    AssertionExecutor,
};

// Re-export test util items
pub use assertion_executor::test_utils::{
    counter_acct_info,
    counter_assertion,
    counter_call,
    COUNTER_ADDRESS as FORK_ADDRESS,
};

/// Deploys the fork test contract.
pub fn deploy_fork_test(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    block_changes: &mut BlockChanges,
) {
    // Fork test contract to block_changes
    block_changes.state_changes.insert(
        FORK_ADDRESS,
        Account {
            info: counter_acct_info(),
            ..Default::default()
        },
    );

    // Deploy fork_test contract
    let transactions = vec![counter_call()];

    let mut fork_db = executor.db.fork();

    let env = BlockEnv {
        basefee: FixedBytes::new([0; 32]).into(),
        ..Default::default()
    };

    transactions
        .into_iter()
        .fold(block_changes, |block_changes, tx_env| {
            let changes = executor
                .execute_forked_tx(env.clone(), tx_env, &mut fork_db)
                .unwrap()
                .1;
            block_changes.state_changes.extend(changes);
            block_changes
        });
}
