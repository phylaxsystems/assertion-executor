use crate::{
    demo_lending::{
        lending_assertion,
        LENDING_ADDRESS,
    },
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

    let fork_assertion = (FORK_ADDRESS, vec![counter_assertion()]);
    let lending_assertion = (LENDING_ADDRESS, vec![lending_assertion()]);
    let assertions = vec![fork_assertion, lending_assertion];

    writer.write_sync(U256::ZERO, assertions).unwrap();

    store
}
