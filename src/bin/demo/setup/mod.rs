mod contracts;
pub use contracts::*;

mod assertion_store;
pub use assertion_store::*;

mod state_deps;
pub use state_deps::*;

mod transactions;
pub use transactions::*;

use assertion_executor::{
    db::SharedDB,
    primitives::BlockChanges,
    AssertionExecutor,
};

use std::path::PathBuf;

/// Deploys contracts, loads assertion store, and sets up state dependencies
pub async fn setup() -> AssertionExecutor<SharedDB<5>> {
    // Remove any existing data directory
    std::fs::remove_dir_all("data").unwrap_or(());

    let db = SharedDB::<5>::new(&PathBuf::from("data")).unwrap();

    let assertion_store_reader = setup_assertion_store();
    let mut executor = ExecutorConfig::default().build(db, assertion_store_reader);

    // Create a block changes object with contract deployments and state deps
    let mut block_changes = BlockChanges::default();

    deploy_contracts(&mut executor, &mut block_changes);
    setup_state_deps(&mut executor, &mut block_changes);
    executor.db.commit_block(block_changes).await.unwrap();

    executor
}
