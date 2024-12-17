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

#[derive(thiserror::Error, Debug)]
pub enum AssertionStoreReaderError {
    #[error("Failed to send request to assertion store")]
    SendError(#[from] mpsc::error::SendError<AssertionStoreReadParams>),
    #[error("Failed to receive response from assertion store")]
    RecvError(#[from] oneshot::error::RecvError),
}

impl AssertionStoreReader {
    /// Matches the given traces from the [`CallTracer`] with the assertions in the assertion store,
    /// ensuring the assertions have been written for the block.
    pub async fn read(
        &mut self,
        block_num: U256,
        traces: CallTracer,
    ) -> Result<Vec<AssertionContract>, AssertionStoreReaderError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.req_tx
            .send(AssertionStoreReadParams {
                block_num,
                traces,
                resp_tx,
            })
            .await?;
        Ok(resp_rx.await?)
    }
}
