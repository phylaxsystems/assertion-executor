use crate::{
    db::{
        fork_db::ForkDb,
        multi_fork_db::MultiForkDb,
        DatabaseRef,
    },
    primitives::{
        uint,
        AssertionContract,
        U256,
    },
    store::{
        map::AssertionStoreMap,
        AssertionStoreReadParams,
        AssertionStoreReader,
        AssertionStoreWriteParams,
    },
    AssertionExecutor,
    AssertionExecutorBuilder,
    ExecutorError,
};
use revm::db::EmptyDB;

use revm::primitives::BlockEnv;

use std::collections::{
    btree_map::Entry,
    BTreeMap,
};
use tokio::sync::mpsc;

// TODO: Support reorgs

/// Synchronization primitive for handling requests to the assertion store
/// Write requests are expected to be in order by block number
/// Match Requests are only processed if the requested block number is less than or equal to the latest block number
pub struct AssertionStoreRequestHandler {
    assertion_store: AssertionStoreMap,
    read_req_rx: mpsc::Receiver<AssertionStoreReadParams>,
    write_req_rx: mpsc::Receiver<AssertionStoreWriteParams>,
    latest_block_num: Option<U256>,
    pending_read_reqs: BTreeMap<U256, Vec<AssertionStoreReadParams>>,
    pending_write_reqs: BTreeMap<U256, AssertionStoreWriteParams>,
    executor: AssertionExecutor<EmptyDB>,
}

#[derive(Debug, thiserror::Error)]
#[error("Failed to send response, receiver dropped")]
pub enum AssertionStoreRequestHandlerError {
    #[error("Failed to send Assertions, receiver dropped")]
    FailedToSendAssertions(#[from] mpsc::error::TrySendError<Vec<AssertionContract>>),
    #[error("Failed to receive requests")]
    FailedToReceiveRequests,
    #[error("Write requests not in order by block number")]
    WriteRequestsNotInOrder,
    #[error("Multiple write requests for the same block number")]
    MultipleWriteRequestsForSameBlock,
    #[error("Assertion executor error")]
    ExecutorError(#[from] ExecutorError<<EmptyDB as DatabaseRef>::Error>),
}

impl AssertionStoreRequestHandler {
    pub fn new(
        read_req_tx: mpsc::Sender<AssertionStoreReadParams>,
        read_req_rx: mpsc::Receiver<AssertionStoreReadParams>,
        write_req_rx: mpsc::Receiver<AssertionStoreWriteParams>,
    ) -> Self {
        let exe_db = EmptyDB::new();
        let reader = AssertionStoreReader {
            req_tx: read_req_tx.clone(),
        };
        let executor = AssertionExecutorBuilder::new(exe_db, reader).build();
        Self {
            assertion_store: AssertionStoreMap::default(),
            pending_read_reqs: BTreeMap::new(),
            pending_write_reqs: BTreeMap::new(),
            read_req_rx,
            executor,
            write_req_rx,
            latest_block_num: None,
        }
    }

    /// Polls the request receiver for new requests related to the assertion store.
    pub async fn poll(&mut self) -> Result<(), AssertionStoreRequestHandlerError> {
        tokio::select! {
            read_req = self.read_req_rx.recv() => {
                if let Some(read_req) = read_req {
                       self.pending_read_reqs.entry(read_req.block_num).or_default().push(read_req);
                       self.process_read_requests()?;
                }
                else {
                    return Err(AssertionStoreRequestHandlerError::FailedToReceiveRequests);
                }

            }
            write_req = self.write_req_rx.recv() => {
                if let Some(AssertionStoreWriteParams { block_num, assertions }) = write_req {
                    match self.pending_write_reqs.entry(block_num) {
                        Entry::Occupied(_) => {
                            return Err(AssertionStoreRequestHandlerError::MultipleWriteRequestsForSameBlock);
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(AssertionStoreWriteParams { assertions, block_num });
                        }
                    }

                    self.process_write_requests()?;
                    self.process_read_requests()?;
                }
                else {
                    return Err(AssertionStoreRequestHandlerError::FailedToReceiveRequests);
                }
            }
        }
        Ok(())
    }
    /// Processes pending write requests
    fn process_write_requests(&mut self) -> Result<(), AssertionStoreRequestHandlerError> {
        let fork = ForkDb::new(self.executor.db);
        while let Some(entry) = self.pending_write_reqs.first_entry() {
            if *entry.key() == Self::valid_block_num(self.latest_block_num) {
                let write_req = entry.remove();

                let AssertionStoreWriteParams {
                    block_num,
                    assertions,
                } = write_req;

                self.latest_block_num = Some(block_num);

                for (addr, assertions) in assertions {
                    let assertion_contracts = self.assertion_store.entry(addr).or_default();
                    for bytecode in assertions {
                        let multi_fork_db = MultiForkDb::new(fork.clone(), fork.clone());
                        let assertion = self.executor.get_assertion_selectors(
                            BlockEnv::default(),
                            bytecode,
                            multi_fork_db,
                        )?;

                        if let Some(assertion) = assertion {
                            assertion_contracts.push(assertion);
                        }
                    }
                }
            } else {
                return Err(AssertionStoreRequestHandlerError::WriteRequestsNotInOrder);
            }
        }

        Ok(())
    }

    /// Processes pending read requests
    fn process_read_requests(&mut self) -> Result<(), AssertionStoreRequestHandlerError> {
        let valid_block_num = Self::valid_block_num(self.latest_block_num);
        while let Some(entry) = self.pending_read_reqs.first_entry() {
            if *entry.key() > valid_block_num {
                break;
            }

            let reqs = entry.remove();

            for req in reqs {
                let AssertionStoreReadParams {
                    traces, resp_tx, ..
                } = req;
                let assertions = self.assertion_store.match_traces(&traces);
                let _ = resp_tx.send(assertions);
            }
        }

        Ok(())
    }

    /// Returns the valid block number for read and write requests
    fn valid_block_num(latest_block_num: Option<U256>) -> U256 {
        latest_block_num
            .map(|latest| latest + uint!(1_U256))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inspectors::tracer::CallTracer,
        primitives::{
            uint,
            Address,
            Bytecode,
        },
        test_utils::*,
    };
    use std::collections::HashSet;
    use tokio::sync::{
        mpsc,
        oneshot,
    };

    fn setup() -> (
        mpsc::Sender<AssertionStoreReadParams>,
        mpsc::Sender<AssertionStoreWriteParams>,
    ) {
        let (read_req_tx, read_req_rx) = mpsc::channel(1000);
        let (write_req_tx, write_req_rx) = mpsc::channel(1000);

        let mut handler =
            AssertionStoreRequestHandler::new(read_req_tx.clone(), read_req_rx, write_req_rx);

        tokio::spawn(async move {
            loop {
                if handler.poll().await.is_err() {
                    break;
                }
            }
        });
        (read_req_tx, write_req_tx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_assertion_store_handler() {
        let (read_req_tx, write_req_tx) = setup();

        let block_num = uint!(0_U256);
        let write_req = AssertionStoreWriteParams {
            block_num,
            assertions: vec![(
                COUNTER_ADDRESS,
                vec![Bytecode::LegacyRaw(bytecode(SIMPLE_ASSERTION_COUNTER))],
            )],
        };

        write_req_tx
            .send(write_req)
            .await
            .expect("Failed to send write request");
        std::thread::sleep(std::time::Duration::from_millis(100));

        let (read_resp_tx, read_resp_rx) = oneshot::channel();
        read_req_tx
            .send(AssertionStoreReadParams {
                block_num: block_num + uint!(1_U256),
                traces: CallTracer {
                    calls: HashSet::from_iter(
                        vec![COUNTER_ADDRESS, Address::new([2u8; 20])].into_iter(),
                    ),
                },
                resp_tx: read_resp_tx,
            })
            .await
            .expect("Failed to send match request");

        let res = read_resp_rx
            .await
            .expect("Failed to receive match response");

        assert_eq!(res, vec![counter_assertion()]);
    }

    #[tokio::test]
    async fn test_assertion_store_handler_blocking_match() {
        use tokio::sync::{
            oneshot,
            oneshot::error::TryRecvError,
        };

        let (read_req_tx, write_req_tx) = setup();

        let (read_resp_tx, mut read_resp_rx) = oneshot::channel();
        read_req_tx
            .send(AssertionStoreReadParams {
                block_num: uint!(1_U256),
                traces: CallTracer {
                    calls: HashSet::from_iter(
                        vec![COUNTER_ADDRESS, Address::new([2u8; 20])].into_iter(),
                    ),
                },
                resp_tx: read_resp_tx,
            })
            .await
            .expect("Failed to send match request");

        assert_eq!(read_resp_rx.try_recv(), Err(TryRecvError::Empty));

        let write_req = AssertionStoreWriteParams {
            block_num: uint!(0_U256),
            assertions: vec![(
                COUNTER_ADDRESS,
                vec![Bytecode::LegacyRaw(bytecode(SIMPLE_ASSERTION_COUNTER))],
            )],
        };

        write_req_tx
            .send(write_req)
            .await
            .expect("Failed to send write request");

        let res = read_resp_rx.await.expect("Failed to receive read response");

        assert_eq!(res, vec![counter_assertion()]);
    }
}
