use assertion_executor::{
    db::SharedDB,
    primitives::BlockChanges,
    AssertionExecutor,
};
/// Executes transactions required to setup state dependencies for the benchmark transactions.
pub fn setup_state_deps(
    _executor: &mut AssertionExecutor<SharedDB<5>>,
    _block_changes: &mut BlockChanges,
) {
}
