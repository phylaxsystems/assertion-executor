use crate::{
    primitives::{
        AssertionContract,
        U256,
    },
    store::{
        map::AssertionStoreMap,
        request::AssertionStoreRequest,
    },
};
use std::collections::HashMap;
use tokio::sync::mpsc;

//TODO: Support reorgs

/// Synchronization primitive for handling requests to the assertion store
/// Write requests are expected to be in order by block number
/// Match Requests are only processed if the requested block number is less than or equal to the latest block number
pub struct AssertionStoreRequestHandler {
    assertion_store: AssertionStoreMap,
    req_rx: mpsc::Receiver<AssertionStoreRequest>,
    latest_block_num: Option<U256>,
    pending_reqs: HashMap<U256, Vec<AssertionStoreRequest>>,
}

#[derive(Debug, thiserror::Error)]
#[error("Failed to send response, receiver dropped")]
pub enum AssertionStoreRequestHandlerError {
    #[error("Failed to send Assertions, receiver dropped")]
    FailedToSendAssertions(#[from] mpsc::error::TrySendError<Vec<AssertionContract>>),
    #[error("Write requests not in order by block number")]
    WriteRequestsNotInOrder,
}

impl AssertionStoreRequestHandler {
    pub fn new(req_rx: mpsc::Receiver<AssertionStoreRequest>) -> Self {
        Self {
            assertion_store: AssertionStoreMap::default(),
            pending_reqs: HashMap::new(),
            req_rx,
            latest_block_num: None,
        }
    }

    pub fn poll(&mut self) -> Result<(), AssertionStoreRequestHandlerError> {
        while let Ok(req) = self.req_rx.try_recv() {
            match req {
                AssertionStoreRequest::Match {
                    block_num,
                    ref traces,
                    ref resp_sender,
                } => {
                    // Check if the requested block number is greater than the latest block number, if so, store the request
                    if block_num > self.latest_block_num.unwrap_or_default() {
                        self.pending_reqs.entry(block_num).or_default().push(req);
                    } else {
                        let assertions = self.assertion_store.match_traces(traces);
                        resp_sender.try_send(assertions)?;
                    }
                }
                AssertionStoreRequest::Write {
                    block_num,
                    assertions,
                } => {
                    if let Some(ref latest_block_num) = self.latest_block_num {
                        if block_num <= *latest_block_num {
                            return Err(AssertionStoreRequestHandlerError::WriteRequestsNotInOrder);
                        }
                    }

                    self.latest_block_num = Some(block_num);

                    for (addr, assertions) in assertions {
                        self.assertion_store.insert(addr, assertions);
                    }

                    // Check if there are any pending match requests for the new block number
                    let pending_reqs = self.pending_reqs.remove(&block_num);
                    if let Some(pending_reqs) = pending_reqs {
                        for pending_req in pending_reqs {
                            if let AssertionStoreRequest::Match {
                                traces,
                                resp_sender,
                                ..
                            } = pending_req
                            {
                                let assertions = self.assertion_store.match_traces(&traces);
                                resp_sender.try_send(assertions)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inspectors::tracer::CallTracer,
        primitives::Address,
    };
    use std::collections::HashSet;

    fn setup() -> mpsc::Sender<AssertionStoreRequest> {
        let (req_tx, req_rx) = mpsc::channel(1000);

        let mut handler = AssertionStoreRequestHandler::new(req_rx);

        std::thread::spawn(move || {
            loop {
                if handler.poll().is_err() {
                    break;
                }
            }
        });
        req_tx
    }

    #[tokio::test]
    async fn test_assertion_store_handler() {
        use crate::test_utils::{
            counter_assertion,
            COUNTER_ADDRESS,
        };
        use revm::primitives::uint;

        let req_tx = setup();

        for block_num in [uint!(0_U256), uint!(1_U256)] {
            let write_req = AssertionStoreRequest::Write {
                block_num,
                assertions: vec![(COUNTER_ADDRESS, vec![counter_assertion()])],
            };

            req_tx
                .send(write_req)
                .await
                .expect("Failed to send write request");

            let (match_resp_tx, mut match_resp_rx) = tokio::sync::mpsc::channel(1000);
            req_tx
                .send(AssertionStoreRequest::Match {
                    block_num,
                    traces: CallTracer {
                        calls: HashSet::from_iter(
                            vec![COUNTER_ADDRESS, Address::new([2u8; 20])].into_iter(),
                        ),
                    },
                    resp_sender: match_resp_tx,
                })
                .await
                .expect("Failed to send match request");

            let res = match_resp_rx
                .recv()
                .await
                .expect("Failed to receive match response");

            assert_eq!(res, vec![counter_assertion()]);
        }
    }

    #[tokio::test]
    async fn test_assertion_store_handler_blocking_match() {
        use crate::test_utils::{
            counter_assertion,
            COUNTER_ADDRESS,
        };
        use revm::primitives::uint;
        use tokio::sync::mpsc::error::TryRecvError;

        let req_tx = setup();

        let (match_resp_tx, mut match_resp_rx) = mpsc::channel(1000);
        req_tx
            .send(AssertionStoreRequest::Match {
                block_num: uint!(1_U256),
                traces: CallTracer {
                    calls: HashSet::from_iter(
                        vec![COUNTER_ADDRESS, Address::new([2u8; 20])].into_iter(),
                    ),
                },
                resp_sender: match_resp_tx,
            })
            .await
            .expect("Failed to send match request");

        assert_eq!(match_resp_rx.try_recv(), Err(TryRecvError::Empty));

        let write_req = AssertionStoreRequest::Write {
            block_num: uint!(1_U256),
            assertions: vec![(COUNTER_ADDRESS, vec![counter_assertion()])],
        };

        req_tx
            .send(write_req)
            .await
            .expect("Failed to send write request");

        let res = match_resp_rx
            .recv()
            .await
            .expect("Failed to receive match response");

        assert_eq!(res, vec![counter_assertion()]);
    }
}
