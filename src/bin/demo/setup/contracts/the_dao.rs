use assertion_executor::{
    db::SharedDB,
    primitives::{
        BlockChanges,
        BlockEnv,
        TxEnv,
    },
    AssertionExecutor,
};
pub fn deploy_the_dao(
    executor: &mut AssertionExecutor<SharedDB<5>>,
    block_changes: &mut BlockChanges,
) {
    // Deploy TheDAO contract
    let transactions = vec![TxEnv::default()];

    let mut fork_db = executor.db.fork();

    transactions
        .into_iter()
        .fold(block_changes, |block_changes, tx_env| {
            let changes = executor
                .execute_forked_tx(BlockEnv::default(), tx_env, &mut fork_db)
                .unwrap()
                .1;
            block_changes.state_changes.extend(changes);
            block_changes
        });
}
