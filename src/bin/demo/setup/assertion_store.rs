use crate::{
    demo_lending::LENDING_ADDRESS,
    fork_test::FORK_ADDRESS,
};

use assertion_executor::{
    store::{
        AssertionState,
        AssertionStore,
    },
    test_utils::*,
};

use super::demo_lending::LENDING;

/// Loads the assertion store with demo assertions
pub async fn setup_assertion_store() -> AssertionStore {
    let store = AssertionStore::new_ephemeral().unwrap();

    store
        .insert(FORK_ADDRESS, AssertionState::new_test(bytecode(COUNTER)))
        .unwrap();
    store
        .insert(LENDING_ADDRESS, AssertionState::new_test(bytecode(LENDING)))
        .unwrap();
    store
}
