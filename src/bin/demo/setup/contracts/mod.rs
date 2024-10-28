mod the_dao;
use the_dao::deploy_the_dao;

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
    [deploy_the_dao].iter().for_each(|f| {
        f(executor, block_changes);
    });
}
