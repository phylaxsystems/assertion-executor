use assertion_executor::{
    primitives::U256,
    store::AssertionStore,
};

/// Loads the assertion store with demo assertions
pub async fn setup_assertion_store() -> AssertionStore {
    let store = AssertionStore::default();
    let writer = store.writer();

    let assertions = vec![];

    writer.write(U256::ZERO, assertions).await.unwrap();

    store
}
