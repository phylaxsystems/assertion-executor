use crate::{
    demo_lending::lending_assertion,
    fork_test::{
        counter_assertion,
        FORK_ADDRESS,
    },
};

use assertion_executor::{
    primitives::U256,
    store::AssertionStore,
};

/// Loads the assertion store with demo assertions
pub fn setup_assertion_store() -> AssertionStore {
    let store = AssertionStore::default();
    let writer = store.writer();

    let assertions = vec![(FORK_ADDRESS, vec![counter_assertion(), lending_assertion()])];

    writer.write_sync(U256::ZERO, assertions).unwrap();

    store
}
