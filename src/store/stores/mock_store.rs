use crate::{
    primitives::{
        Address,
        AssertionContract,
        Bytecode,
        B256,
    },
    store::{
        assertion_contract_extractor::{
            AssertionContractExtractor,
            FnSelectorExtractorError,
        },
        AssertionStoreReadParams,
        AssertionStoreReader,
    },
    ExecutorConfig,
};

use tracing::{
    error,
    trace,
};

use std::collections::HashMap;
use tokio::{
    sync::mpsc,
    task::JoinHandle,
};

/// An in-memory assertion store.
/// This allows you to manually insert assertions into the store.
/// All assertions will be available for reading regardless of block number specified in the read
/// request.
/// ``` rust
/// use assertion_executor::{
///     primitives::{
///         Address,
///         Bytecode,
///         SpecId,
///     },
///     store::{
///         AssertionStoreReader,
///         MockStore,
///     },
///     ExecutorConfig,
/// };
///
/// #[tokio::main]
/// async fn main() {
///     // Create executor config
///     let executor_config = ExecutorConfig {
///         chain_id: 1,
///         spec_id: SpecId::LATEST,
///         assertion_gas_limit: 1_000_000,
///     };
///
///     let mut store = MockStore::new(executor_config);
///     // Populate store
///     store.insert(Address::default(), vec![Bytecode::default()]);
///
///     // Spawn read request handler, and get reader for use in `AssertionExecutor`
///     let reader: AssertionStoreReader = store.reader();
/// }
/// ```
#[derive(Debug, Default)]
pub struct MockStore {
    /// Stores assertions in memory
    store: HashMap<Address, Vec<AssertionContract>>,
    /// Used for deploying bytecode, executing fn_selectors, and forming an [`AssertionContract`]
    assertion_contract_extractor: AssertionContractExtractor,
}

impl MockStore {
    pub fn new(executor_config: ExecutorConfig) -> Self {
        Self {
            store: HashMap::new(),
            assertion_contract_extractor: AssertionContractExtractor::new(executor_config),
        }
    }

    /// Inserts a  group of assertions contract to be read from blocks > 0, into the store.
    /// Returns the previous contract group if it existed.
    pub fn insert(
        &mut self,
        address: Address,
        assertions: Vec<Bytecode>,
    ) -> Result<Option<Vec<AssertionContract>>, FnSelectorExtractorError> {
        let mut assertion_contracts = Vec::new();
        for assertion in assertions {
            let assertion_contract = self
                .assertion_contract_extractor
                .extract_assertion_contract(assertion)?;
            assertion_contracts.push(assertion_contract);
        }

        Ok(self.store.insert(address, assertion_contracts))
    }

    /// Spawns a read request handler and returns a join handle and reader.
    pub fn reader(self) -> AssertionStoreReader {
        self.cancellable_reader(tokio_util::sync::CancellationToken::new())
            .0
    }

    /// Spawns a read request handler and returns a join handle and reader.
    pub fn cancellable_reader(
        self,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) -> (AssertionStoreReader, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(1_000);

        let mut store = MockStoreRequestHandler {
            store: self.store,
            rx,
        };

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = store.handle_read_req() => {},
                    _ = cancellation_token.cancelled() =>  break,
                }
            }
        });

        (AssertionStoreReader::new(tx.clone()), handle)
    }
}

/// Handles read requests from the reader
struct MockStoreRequestHandler {
    /// Stores assertions in memory
    store: HashMap<Address, Vec<AssertionContract>>,
    /// Receives read requests from the Reader
    rx: mpsc::Receiver<AssertionStoreReadParams>,
}

impl MockStoreRequestHandler {
    /// Handles read requests from the reader
    async fn handle_read_req(&mut self) {
        let read_req = self.rx.recv().await;
        if let Some(read_req) = read_req {
            let AssertionStoreReadParams {
                traces, resp_tx, ..
            } = read_req;

            let assertion_contracts = traces
                .calls()
                .iter()
                .filter_map(|to: &Address| {
                    let maybe_assertions = self.store.get(to).cloned();

                    if let Some(ref assertions) = maybe_assertions {
                        let assertion_hashes: Vec<B256> = assertions
                            .iter()
                            .map(|assertion: &AssertionContract| assertion.code_hash)
                            .collect();
                        trace!(?assertion_hashes, contract = ?to, "Found assertions for contract");
                    };

                    maybe_assertions
                })
                .flatten()
                .collect::<Vec<AssertionContract>>();

            let _ = resp_tx.send(assertion_contracts);
        } else {
            error!("Failed to receive requests");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inspectors::tracer::CallTracer,
        test_utils::{
            bytecode,
            random_bytes,
            FN_SELECTOR,
        },
    };
    use revm::interpreter::{
        CallInputs,
        CallScheme,
    };
    use revm::primitives::{
        Bytes,
        FixedBytes,
    };

    #[test]
    fn test_insert() {
        let mut store = MockStore::default();
        let address = Address::default();
        let assertion = Bytecode::LegacyRaw(bytecode(FN_SELECTOR));

        // Test insert
        assert!(store
            .insert(address, vec![assertion.clone()])
            .unwrap()
            .is_none());

        // Test inserting to same address returns previous value
        let previous = store.insert(address, vec![assertion.clone()]).unwrap();
        assert!(previous.is_some());
    }

    #[tokio::test]
    async fn test_read_request_handler() {
        let mut store = MockStore::default();

        // Create test data
        let address = random_bytes().into();

        store
            .insert(address, vec![Bytecode::LegacyRaw(bytecode(FN_SELECTOR))])
            .unwrap();

        // Spawn the handler
        let mut reader = store.reader();

        // Create read request parameters
        let traces = CallTracer {
            call_inputs: HashMap::from_iter([(
                (address, FixedBytes::default()),
                vec![CallInputs {
                    input: Bytes::default(),
                    return_memory_offset: 0..0,
                    gas_limit: 0,
                    bytecode_address: address,
                    target_address: address,
                    caller: Address::default(),
                    value: Default::default(),
                    scheme: CallScheme::Call,
                    is_static: false,
                    is_eof: false,
                }],
            )]),
        };

        // Send read request
        let result = reader.read(Default::default(), traces).await;
        assert!(result.is_ok());

        let assertions = result.unwrap();
        assert_eq!(assertions.len(), 1);
    }

    #[tokio::test]
    async fn test_cancellable_reader() {
        let mut store = MockStore::default();

        // Create test data
        let address = random_bytes().into();

        store
            .insert(address, vec![Bytecode::LegacyRaw(bytecode(FN_SELECTOR))])
            .unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        // Spawn the handler
        let (mut reader, handle) = store.cancellable_reader(token.clone());

        // Create read request parameters
        let traces = CallTracer {
            call_inputs: HashMap::from_iter([(
                (address, FixedBytes::default()),
                vec![CallInputs {
                    input: Bytes::default(),
                    return_memory_offset: 0..0,
                    gas_limit: 0,
                    bytecode_address: address,
                    target_address: address,
                    caller: Address::default(),
                    value: Default::default(),
                    scheme: CallScheme::Call,
                    is_static: false,
                    is_eof: false,
                }],
            )]),
        };

        // Send read request
        let result = reader.read(Default::default(), traces.clone()).await;
        assert!(result.is_ok());

        let assertions = result.unwrap();
        assert_eq!(assertions.len(), 1);

        // Cancel the reader
        token.cancel();
        let _ = handle.await;

        // Send read request
        // This should not be processed
        let result = reader.read(Default::default(), traces).await;
        assert!(result.is_err());
    }
}
