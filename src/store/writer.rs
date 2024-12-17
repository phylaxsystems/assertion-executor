use crate::primitives::{
    Address,
    Bytecode,
    U256,
};

use tokio::sync::mpsc;

/// A writer for writing block based batches to the [`AssertionStore`](super::AssertionStore).
pub struct AssertionStoreWriter {
    pub(super) req_tx: mpsc::Sender<AssertionStoreWriteParams>,
}

/// Parameters for writing assertions to the assertion store.
#[derive(Debug)]
pub struct AssertionStoreWriteParams {
    pub block_num: U256,
    pub assertions: Vec<(Address, Vec<Bytecode>)>,
}

impl AssertionStoreWriter {
    /// Writes a batch of assertions to the [`AssertionStore`](super::AssertionStore) for a given block number.
    /// Must be called in order of block number.
    pub async fn write(
        &self,
        block_num: U256,
        assertions: Vec<(Address, Vec<Bytecode>)>,
    ) -> Result<(), mpsc::error::SendError<AssertionStoreWriteParams>> {
        self.req_tx
            .send(AssertionStoreWriteParams {
                block_num,
                assertions,
            })
            .await
    }
}
