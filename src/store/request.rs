use crate::{
    primitives::{
        Address,
        AssertionContract,
        U256,
    },
    tracer::CallTracer,
};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum AssertionStoreRequest {
    Match {
        block_num: U256,
        traces: CallTracer,
        resp_sender: mpsc::Sender<Vec<AssertionContract>>,
    },
    Write {
        block_num: U256,
        assertions: Vec<(Address, Vec<AssertionContract>)>,
    },
}
