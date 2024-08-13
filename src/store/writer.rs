use crate::{
    primitives::{
        Address,
        AssertionContract,
        U256,
    },
    store::request::AssertionStoreRequest,
};
use tokio::sync::mpsc;

/// A writer for writing block based batches to the [`AssertionStore`](super::AssertionStore).
pub struct AssertionStoreWriter {
    pub(super) req_tx: mpsc::Sender<AssertionStoreRequest>,
}
impl AssertionStoreWriter {
    /// Writes a batch of assertions to the [`AssertionStore`](super::AssertionStore) for a given block number.
    /// Must be called in order of block number.
    pub async fn write(
        &self,
        block_num: U256,
        assertions: Vec<(Address, Vec<AssertionContract>)>,
    ) -> Result<(), mpsc::error::SendError<AssertionStoreRequest>> {
        self.req_tx
            .send(AssertionStoreRequest::Write {
                block_num,
                assertions,
            })
            .await
    }
}
