use crate::{
    demo_lending::LENDING_ADDRESS,
    fork_test::FORK_ADDRESS,
};

use assertion_executor::{
    primitives::{
        Bytecode,
        U256,
    },
    store::AssertionStore,
    test_utils::*,
};

use super::demo_lending::LENDING;

/// Loads the assertion store with demo assertions
pub async fn setup_assertion_store() -> AssertionStore {
    let store = AssertionStore::default();
    let writer = store.writer();

    let fork_assertion = (FORK_ADDRESS, vec![Bytecode::LegacyRaw(bytecode(COUNTER))]);
    let lending_assertion = (
        LENDING_ADDRESS,
        vec![Bytecode::LegacyRaw(bytecode(LENDING))],
    );
    let assertions = vec![fork_assertion, lending_assertion];

    writer.write(U256::ZERO, assertions).await.unwrap();

    store
}
