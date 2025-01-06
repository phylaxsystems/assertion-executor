use crate::{
    inspectors::tracer::CallTracer,
    primitives::{
        AssertionContract,
        U256,
    },
};
use std::time::Duration;
use tokio::{
    sync::{
        mpsc,
        oneshot,
    },
    time::timeout,
};

/// A reader for matching [`CallTracer`] against assertions in the [`AssertionStore`](crate::store::AssertionStore) at a specific block.
#[derive(Debug, Clone)]
pub struct AssertionStoreReader {
    pub(super) req_tx: mpsc::Sender<AssertionStoreReadParams>,
    /// Timeout duration for read operations,
    timeout: Duration,
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
    #[error("Timeout waiting for response after {0:?}")]
    Timeout(Duration),
}

impl AssertionStoreReader {
    /// Creates a new reader with the specified timeout
    pub fn new(req_tx: mpsc::Sender<AssertionStoreReadParams>, timeout: Duration) -> Self {
        Self { req_tx, timeout }
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

        // Wait for response with timeout
        match timeout(self.timeout, resp_rx).await {
            Ok(result) => Ok(result?),
            Err(_elapsed) => Err(AssertionStoreReaderError::Timeout(self.timeout)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_reader_timeout() {
        // Create channels with small buffer
        let (tx, mut rx) = mpsc::channel(1);

        // Create reader with very short timeout
        let mut reader = AssertionStoreReader::new(tx, Duration::from_millis(100));

        // Spawn a task that deliberately delays processing the request
        tokio::spawn(async move {
            let params = rx.recv().await.unwrap();
            sleep(Duration::from_secs(1)).await; // Delay longer than timeout
            let _ = params.resp_tx.send(vec![]); // This send will happen after timeout
        });

        // Attempt to read with timeout
        let result = reader.read(U256::ZERO, CallTracer::default()).await;

        assert!(matches!(result, Err(AssertionStoreReaderError::Timeout(_))));
    }

    #[tokio::test]
    async fn test_reader_success() {
        // Create channels
        let (tx, mut rx) = mpsc::channel(1);

        // Create reader with reasonable timeout
        let mut reader = AssertionStoreReader::new(tx, Duration::from_secs(1));

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
