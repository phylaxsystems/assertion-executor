use crate::{
    inspectors::tracer::CallTracer,
    primitives::{
        AssertionContract,
        U256,
    },
};
use tokio::sync::{
    mpsc,
    oneshot,
};

/// A reader for matching [`CallTracer`] against assertions in the [`AssertionStore`](crate::store::AssertionStore) at a specific block.
#[derive(Debug, Clone)]
pub struct AssertionStoreReader {
    pub(super) req_tx: mpsc::Sender<AssertionStoreReadParams>,
}

/// Parameters for reading assertions from the assertion store.
#[derive(Debug)]
pub struct AssertionStoreReadParams {
    pub block_num: U256,
    pub traces: CallTracer,
    pub resp_tx: oneshot::Sender<Vec<AssertionContract>>,
}

impl PartialEq for AssertionStoreReadParams {
    fn eq(&self, other: &Self) -> bool {
        self.block_num == other.block_num && self.traces == other.traces
    }
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum AssertionStoreReaderError {
    #[error("Failed to send request to assertion store")]
    SendError(#[from] mpsc::error::SendError<AssertionStoreReadParams>),
    #[error("Failed to receive response from assertion store")]
    RecvError(#[from] oneshot::error::RecvError),
}

impl AssertionStoreReader {
    /// Creates a new reader
    pub fn new(req_tx: mpsc::Sender<AssertionStoreReadParams>) -> Self {
        Self { req_tx }
    }

    /// Matches the given traces from the [`CallTracer`] with the assertions in the assertion store,
    /// ensuring the assertions have been written for the block.
    ///
    /// Returns an error if the operation times out after the configured duration.
    pub async fn read(
        &mut self,
        block_num: U256,
        traces: CallTracer,
    ) -> Result<Vec<AssertionContract>, AssertionStoreReaderError> {
        let (resp_tx, resp_rx) = oneshot::channel();

        // Send the request
        self.req_tx
            .send(AssertionStoreReadParams {
                block_num,
                traces,
                resp_tx,
            })
            .await?;

        // Wait for response
        resp_rx.await.map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_reader_success() {
        // Create channels
        let (tx, mut rx) = mpsc::channel(1);

        let mut reader = AssertionStoreReader::new(tx);

        // Spawn a task that processes the request immediately
        tokio::spawn(async move {
            let params = rx.recv().await.unwrap();
            let _ = params.resp_tx.send(vec![AssertionContract::default()]);
        });

        // Attempt to read
        let result = reader.read(U256::ZERO, CallTracer::default()).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }
}
