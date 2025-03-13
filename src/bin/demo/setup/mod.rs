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
    ExecutorConfig,
};

use alloy_provider::Provider;

use std::path::PathBuf;

/// Deploys contracts, loads assertion store, and sets up state dependencies
pub async fn setup() -> AssertionExecutor<SharedDB<5>> {
    // Remove any existing data directory
    std::fs::remove_dir_all("data").unwrap_or(());

    use alloy_node_bindings::Anvil;
    use alloy_provider::{
        ProviderBuilder,
        WsConnect,
    };

    let anvil = Anvil::new().spawn();
    let provider = ProviderBuilder::new()
        .on_ws(WsConnect::new(anvil.ws_endpoint()))
        .await
        .unwrap();

    #[allow(deprecated)]
    let provider = provider.root().clone().boxed();

    let db = SharedDB::<5>::new(&PathBuf::from("data"), provider, Default::default()).unwrap();

    let assertion_store = setup_assertion_store().await;
    let mut executor = ExecutorConfig::default().build(db, assertion_store);

    // Create a block changes object with contract deployments and state deps
    let mut block_changes = BlockChanges::default();

    deploy_contracts(&mut executor, &mut block_changes);
    setup_state_deps(&mut executor, &mut block_changes);
    executor.db.commit_block(block_changes).unwrap();

    executor
}
