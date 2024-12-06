mod handler;
mod map;

mod request;
pub use request::AssertionStoreRequest;

mod reader;
pub use reader::AssertionStoreReader;

mod writer;
pub use writer::AssertionStoreWriter;

use tokio::sync::mpsc;
use tracing::error;

/// Used for synchronizing reads and writes of assertions by block number.
///
/// Writes are processed order by block number
/// Read Requests are only processed if the requested block number is less than or equal to the latest block number
//
// # Example
//
//```
// #[tokio::main]
// async fn main() {
//     use assertion_executor::{
//         inspectors::tracer::CallTracer,
//         primitives::{
//             Address,
//             AssertionContract,
//             U256,
//         },
//         store::AssertionStore,
//     };
//
//     let assertion_store = AssertionStore::default();
//
//     let writer = assertion_store.writer();
//     let mut reader = assertion_store.reader();
//
//     let mut trace = CallTracer::default();
//     trace.calls.insert(Address::new([0; 20]));
//
//     assert_eq!(
//         reader
//             .read(U256::ZERO, trace.clone())
//             .await
//             .unwrap()
//             .unwrap(),
//         vec![]
//     );
//
//     writer
//         .write(
//             U256::ZERO,
//             vec![(Address::new([0; 20]), vec![assertion_executor::primitives::Bytecode::default()])],
//         )
//         .await;
//
//     assert_eq!(
//         reader.read(U256::ZERO, trace).await.unwrap().unwrap(),
//         vec![AssertionContract::default()]
//     );
// }
// ```
#[derive(Debug)]
pub struct AssertionStore {
    req_tx: mpsc::Sender<AssertionStoreRequest>,
    reader_channel_size: usize,
}

impl AssertionStore {
    /// Instantiates the [`AssertionStore`], spawning the handler polling on a new thread, and returns the [`AssertionStore`].
    pub fn new(req_channel_size: usize, reader_channel_size: usize) -> Self {
        let (req_tx, req_rx) = mpsc::channel(req_channel_size);

        std::thread::spawn(move || {
            let mut req_handler = handler::AssertionStoreRequestHandler::new(req_rx);

            loop {
                if let Err(err) = req_handler.poll() {
                    // TODO: tokio selectify errors
                    error!(
                        ?err,
                        "AssertionStore: Error polling assertion store handler"
                    );
                }
            }
        });

        AssertionStore {
            reader_channel_size,
            req_tx,
        }
    }

    /// Instantiates an [`AssertionStoreReader`] and returns it.
    pub fn reader(&self) -> AssertionStoreReader {
        let (match_tx, match_rx) = mpsc::channel(self.reader_channel_size);
        AssertionStoreReader {
            req_tx: self.req_tx.clone(),
            match_rx,
            match_tx,
        }
    }

    /// Instantiates an [`AssertionStoreWriter`] and returns it.
    pub fn writer(&self) -> AssertionStoreWriter {
        AssertionStoreWriter {
            req_tx: self.req_tx.clone(),
        }
    }
}

impl Default for AssertionStore {
    fn default() -> Self {
        Self::new(1_000, 100)
    }
}
