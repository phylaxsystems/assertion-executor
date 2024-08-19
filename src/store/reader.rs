use crate::{
    primitives::{
        AssertionContract,
        U256,
    },
    store::request::AssertionStoreRequest,
    tracer::CallTracer,
};
use tokio::sync::mpsc;

/// A reader for matching [`CallTracer`] against assertions in the [`AssertionStore`](crate::store::AssertionStore) at a specific block.
#[derive(Debug)]
pub struct AssertionStoreReader {
    pub(super) req_tx: mpsc::Sender<AssertionStoreRequest>,
    pub(super) match_tx: mpsc::Sender<Vec<AssertionContract>>,
    pub(super) match_rx: mpsc::Receiver<Vec<AssertionContract>>,
}

impl Clone for AssertionStoreReader {
    fn clone(&self) -> Self {
        let (match_tx, match_rx) = mpsc::channel(self.match_rx.max_capacity());
        Self {
            req_tx: self.req_tx.clone(),
            match_rx,
            match_tx,
        }
    }
}

impl AssertionStoreReader {
    /// Matches the given traces from the [`CallTracer`] with the assertions in the assertion store,
    /// ensuring the assertions have been written for the block.
    pub async fn read(
        &mut self,
        block_num: U256,
        traces: CallTracer,
    ) -> Result<Option<Vec<AssertionContract>>, mpsc::error::SendError<AssertionStoreRequest>> {
        self.req_tx
            .send(AssertionStoreRequest::Match {
                block_num,
                traces,
                resp_sender: self.match_tx.clone(),
            })
            .await?;
        Ok(self.match_rx.recv().await)
    }

    /// Matches the given traces from the [`CallTracer`] with the assertions in the assertion store,
    /// ensuring the assertions have been written for the block.
    /// For use in synchronous contexts, will fail in async runtimes.
    pub fn read_sync(
        &mut self,
        block_num: U256,
        traces: CallTracer,
    ) -> Result<Option<Vec<AssertionContract>>, mpsc::error::SendError<AssertionStoreRequest>> {
        self.req_tx.blocking_send(AssertionStoreRequest::Match {
            block_num,
            traces,
            resp_sender: self.match_tx.clone(),
        })?;
        Ok(self.match_rx.blocking_recv())
    }
}
