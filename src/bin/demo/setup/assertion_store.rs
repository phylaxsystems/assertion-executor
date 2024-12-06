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
    test_utils::COUNTER_CODE,
};

use super::demo_lending::LENDING_CODE;

/// Loads the assertion store with demo assertions
pub fn setup_assertion_store() -> AssertionStore {
    let store = AssertionStore::default();
    let writer = store.writer();

    let fork_assertion = (FORK_ADDRESS, vec![Bytecode::LegacyRaw(COUNTER_CODE)]);
    let lending_assertion = (LENDING_ADDRESS, vec![Bytecode::LegacyRaw(LENDING_CODE)]);
    let assertions = vec![fork_assertion, lending_assertion];

    writer.write_sync(U256::ZERO, assertions).unwrap();

    store
}
