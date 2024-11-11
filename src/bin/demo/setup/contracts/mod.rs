//! `contracts
//!
//! The following mod contains setup code to deploy contracts for the benchmark.
//! All code in the benchmarks can be found in `/contract-mocks` at the root of
//! this repository.

pub mod demo_lending;
pub mod fork_test;
use demo_lending::deploy_demo_lending;
use fork_test::deploy_fork_test;

use assertion_executor::{
    db::SharedDB,
    primitives::BlockChanges,
    AssertionExecutor,
};

pub fn deploy_contracts(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    block_changes: &mut BlockChanges,
) {
    // Deploy contracts
    [deploy_fork_test, deploy_demo_lending]
        .iter()
        .for_each(|f| {
            f(executor, block_changes);
        });
}
