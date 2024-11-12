use crate::CALLER;
use assertion_executor::{
    db::SharedDB,
    primitives::{
        Account,
        AccountInfo,
        BlockChanges,
        U256,
    },
    AssertionExecutor,
};
/// Executes transactions required to setup state dependencies for the benchmark transactions.
pub fn setup_state_deps(
    _executor: &mut AssertionExecutor<SharedDB<5>>,
    block_changes: &mut BlockChanges,
) {
    block_changes.state_changes.insert(
        CALLER,
        Account {
            info: AccountInfo {
                balance: U256::MAX,
                ..Default::default()
            },
            ..Default::default()
        },
    );
}
