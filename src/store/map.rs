use crate::{
    primitives::{
        Address,
        AssertionContract,
        B256,
    },
    tracer::CallTracer,
};

use std::collections::HashMap;
use tracing::debug;

#[derive(Default)]
pub struct AssertionStoreMap(HashMap<Address, Vec<AssertionContract>>);

impl AssertionStoreMap {
    pub fn match_traces(&self, traces: &CallTracer) -> Vec<AssertionContract> {
        let assertion_contracts = traces
            .calls
            .iter()
            .filter_map(move |to: &Address| {
                let maybe_assertions = self.0.get(to).cloned();

                if let Some(ref assertions) = maybe_assertions {
                    let assertion_hashes: Vec<B256> = assertions
                        .iter()
                        .map(|assertion: &AssertionContract| assertion.code_hash)
                        .collect();
                    debug!(?assertion_hashes, contract = ?to, "Found assertions for contract");
                };

                maybe_assertions
            })
            .flatten()
            .collect::<Vec<AssertionContract>>();

        assertion_contracts
    }

    pub fn insert(&mut self, address: Address, assertions: Vec<AssertionContract>) {
        self.0.insert(address, assertions);
    }
}

#[test]
fn test_assertion_store_map() {
    use std::collections::HashSet;
    let mut store = AssertionStoreMap::default();

    let address = Address::new([1u8; 20]);
    let assertion = crate::test_utils::counter_assertion();

    store.insert(address, vec![assertion.clone()]);

    let traces = CallTracer {
        calls: HashSet::from_iter(vec![address, Address::new([2u8; 20])]),
    };

    let matched_assertions = store.match_traces(&traces);
    assert_eq!(matched_assertions.len(), 1);
    assert_eq!(matched_assertions[0], assertion);
}
