mod handler;
mod map;

mod reader;
pub use reader::{
    AssertionStoreReadParams,
    AssertionStoreReader,
    AssertionStoreReaderError,
};

pub mod stores;

mod writer;
pub use writer::{
    AssertionStoreWriteParams,
    AssertionStoreWriter,
};

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
    read_req_tx: mpsc::Sender<AssertionStoreReadParams>,
    write_req_tx: mpsc::Sender<AssertionStoreWriteParams>,
}

impl AssertionStore {
    /// Instantiates the [`AssertionStore`], spawning the handler polling on a new thread, and returns the [`AssertionStore`].
    pub fn new(req_channel_size: usize) -> Self {
        let (read_req_tx, read_req_rx) = mpsc::channel(req_channel_size);
        let (write_req_tx, write_req_rx) = mpsc::channel(req_channel_size);

        let read_req_tx_clone = read_req_tx.clone();
        tokio::spawn(async {
            let mut req_handler = handler::AssertionStoreRequestHandler::new(
                read_req_tx_clone,
                read_req_rx,
                write_req_rx,
            );

            loop {
                if let Err(err) = req_handler.poll().await {
                    // TODO: tokio selectify errors
                    error!(
                        ?err,
                        "AssertionStore: Error polling assertion store handler"
                    );
                }
            }
        });

        AssertionStore {
            read_req_tx,
            write_req_tx,
        }
    }

    /// Instantiates an [`AssertionStoreReader`] and returns it.
    pub fn reader(&self) -> AssertionStoreReader {
        AssertionStoreReader {
            req_tx: self.read_req_tx.clone(),
        }
    }

    /// Instantiates an [`AssertionStoreWriter`] and returns it.
    pub fn writer(&self) -> AssertionStoreWriter {
        AssertionStoreWriter {
            req_tx: self.write_req_tx.clone(),
        }
    }
}

impl Default for AssertionStore {
    fn default() -> Self {
        Self::new(1_000)
    }
}
