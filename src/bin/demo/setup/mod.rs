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
    AssertionExecutorBuilder,
};

use std::path::PathBuf;

/// Deploys contracts, loads assertion store, and sets up state dependencies
pub fn setup() -> AssertionExecutor<SharedDB<5>> {
    // Remove any existing data directory
    std::fs::remove_dir_all("data").unwrap_or(());

    let db = SharedDB::<5>::new(&PathBuf::from("data")).unwrap();

    let assertion_store = setup_assertion_store();
    let mut executor = AssertionExecutorBuilder::new(db, assertion_store.reader()).build();

    // Create a block changes object with contract deployments and state deps
    let mut block_changes = BlockChanges::default();

    deploy_contracts(&mut executor, &mut block_changes);
    setup_state_deps(&mut executor, &mut block_changes);
    let _ = executor.db.commit_block(block_changes).unwrap();

    executor
}
