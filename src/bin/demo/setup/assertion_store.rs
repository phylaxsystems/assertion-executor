use crate::{
    demo_lending::LENDING_ADDRESS,
    fork_test::FORK_ADDRESS,
};

use assertion_executor::{
    primitives::Bytecode,
    store::{
        AssertionStoreReader,
        MockStore,
    },
    test_utils::*,
};

use super::demo_lending::LENDING;

/// Loads the assertion store with demo assertions
pub fn setup_assertion_store() -> AssertionStoreReader {
    let mut store = MockStore::default();

    store
        .insert(FORK_ADDRESS, vec![Bytecode::LegacyRaw(bytecode(COUNTER))])
        .unwrap();
    store
        .insert(
            LENDING_ADDRESS,
            vec![Bytecode::LegacyRaw(bytecode(LENDING))],
        )
        .unwrap();

    store.reader()
}
