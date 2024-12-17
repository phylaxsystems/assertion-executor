use crate::{
    db::{
        fork_db::ForkDb,
        multi_fork_db::MultiForkDb,
        SharedDB,
    },
    executor::FnSelectorsError,
    executor::{
        decode_selector_array,
        fnSelectorsCall,
        ASSERTION_CONTRACT,
        CALLER,
    },
    inspectors::phevm::{
        insert_precompile_account,
        PhEvmInspector,
    },
    primitives::{
        uint,
        Address,
        AssertionContract,
        Bytecode,
        ExecutionResult,
        TxEnv,
        TxKind,
        U256,
    },
    store::{
        map::AssertionStoreMap,
        AssertionStoreReadParams,
        AssertionStoreWriteParams,
    },
};

use alloy_sol_types::SolCall;
use revm::{
    inspector_handle_register,
    primitives::{
        BlockEnv,
        SpecId,
    },
    DatabaseCommit,
    Evm,
    EvmBuilder,
};

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
    #[error("Error processing assertion selectors")]
    AssertionSelectorError(#[from] FnSelectorsError),
}

impl AssertionStoreRequestHandler {
    pub fn new(
        read_req_rx: mpsc::Receiver<AssertionStoreReadParams>,
        write_req_rx: mpsc::Receiver<AssertionStoreWriteParams>,
    ) -> Self {
        Self {
            assertion_store: AssertionStoreMap::default(),
            pending_read_reqs: BTreeMap::new(),
            pending_write_reqs: BTreeMap::new(),
            read_req_rx,
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
        while let Some(entry) = self.pending_write_reqs.first_entry() {
            if *entry.key() == Self::valid_block_num(self.latest_block_num) {
                let write_req = entry.remove();

                let AssertionStoreWriteParams {
                    block_num,
                    assertions,
                } = write_req;

                self.latest_block_num = Some(block_num);

                let forkb = SharedDB::<0>::new_ephemeral().unwrap();

                for (addr, assertions) in assertions {
                    let mut assertion_contracts = vec![];
                    for bytecode in assertions {
                        let multi_fork_db = MultiForkDb::new(forkb.fork(), forkb.fork());
                        let assertion =
                            get_assertion_selectors(bytecode, BlockEnv::default(), multi_fork_db)?;
                        assertion_contracts.push(assertion);
                    }

                    self.assertion_store.insert(addr, assertion_contracts);
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

/// As a part of the spec, assertions mus implement the `fnSelectors()` function.
/// The function returns `Trigger[] memory` where `Trigger` is a:
/// ```javascript
/// enum TriggerType {
///     STORAGE,
///     ETHER,
///     BOTH
/// }
///
/// struct Trigger {
///     TriggerType triggerType;
///     bytes4 fnSelector;
/// }
/// ```
/// This function executes the `fnSelectors` function and returns an `AssertionContract`.
/// If the function is unimplemented, or otherwise errors, it returns `None`.
fn get_assertion_selectors<DB: revm::DatabaseRef + std::clone::Clone>(
    assertion_code: Bytecode,
    block_env: BlockEnv,
    mut fork_db: MultiForkDb<ForkDb<DB>>,
) -> Result<AssertionContract, FnSelectorsError> {
    // Deploy the contract first
    let assertion_address =
        deploy_assertion_contract(block_env.clone(), assertion_code.clone(), &mut fork_db)
            .ok_or(FnSelectorsError::EvmError)?;

    // Set up and execute the call
    let result = EvmBuilder::default()
        .with_db(fork_db)
        .modify_db(insert_precompile_account)
        .with_external_context(PhEvmInspector::new(SpecId::LATEST))
        .with_spec_id(SpecId::LATEST)
        .with_block_env(block_env)
        .modify_tx_env(|env| {
            *env = TxEnv {
                transact_to: TxKind::Call(assertion_address),
                caller: CALLER,
                data: fnSelectorsCall::SELECTOR.into(),
                ..Default::default()
            }
        })
        .append_handler_register(inspector_handle_register)
        .build()
        .transact()
        .map_err(|_| FnSelectorsError::EvmError)?;

    // Extract and decode selectors from the result
    let selectors = match result.result {
        ExecutionResult::Success { output, .. } => decode_selector_array(&output)?,
        other => return Err(FnSelectorsError::UnsuccessfulExecution(other)),
    };

    Ok(AssertionContract {
        code_hash: assertion_code.hash_slow(),
        code: assertion_code,
        fn_selectors: selectors,
    })
}

/// Deploys an assertion contract to the forked db.
/// Returns the address of the deployed contract.
fn deploy_assertion_contract<DB: revm::DatabaseRef + std::clone::Clone>(
    block_env: BlockEnv,
    constructor_code: Bytecode,
    multi_fork_db: &mut MultiForkDb<ForkDb<DB>>,
) -> Option<Address> {
    let tx_env = TxEnv {
        transact_to: TxKind::Create,
        caller: CALLER,
        data: constructor_code.original_bytes(),
        ..Default::default()
    };

    let mut evm = Evm::builder()
        .with_db(multi_fork_db.clone())
        .modify_db(insert_precompile_account)
        .with_external_context(PhEvmInspector::new(SpecId::LATEST))
        .with_spec_id(SpecId::LATEST)
        .with_block_env(BlockEnv {
            basefee: U256::ZERO,
            ..block_env
        })
        .modify_tx_env(|env| *env = tx_env)
        .append_handler_register(inspector_handle_register)
        .build();

    let result = match evm.transact() {
        Ok(rax) => rax,
        Err(_) => return None,
    };

    multi_fork_db.commit(result.state.clone());

    Some(ASSERTION_CONTRACT)
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

        let mut handler = AssertionStoreRequestHandler::new(read_req_rx, write_req_rx);

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

    #[test]
    fn test_get_assertion_selectors() {
        let shared_db = SharedDB::<0>::new_test();

        let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());

        // Test with valid assertion contract
        let assertion_contract = get_assertion_selectors(
            Bytecode::LegacyRaw(bytecode(FN_SELECTOR)),
            BlockEnv::default(),
            multi_fork_db.clone(),
        )
        .unwrap();

        // Verify the contract has the expected selectors from the counter assertion
        let expected_assertion = selector_assertion();
        assert_eq!(
            assertion_contract.fn_selectors,
            expected_assertion.fn_selectors
        );
        assert_eq!(assertion_contract.code_hash, expected_assertion.code_hash);

        // Test with invalid return
        let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());
        let result = get_assertion_selectors(
            Bytecode::LegacyRaw(bytecode(BAD_FN_SELECTOR)),
            BlockEnv::default(),
            multi_fork_db,
        );

        // Should return None for invalid code
        assert!(result.is_err(), "Should return error");

        // Test with empty code
        let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());
        let result = get_assertion_selectors(
            Bytecode::LegacyRaw(vec![].into()),
            BlockEnv::default(),
            multi_fork_db,
        );

        // Should return None for empty code
        assert!(result.is_err(), "Should return Error for empty code");
    }

    #[test]
    fn test_get_assertion_selectors_with_different_block_envs() {
        let shared_db = SharedDB::<0>::new_test();

        // Test with different block numbers
        for block_number in [0u64, 1u64, 1000u64].map(U256::from) {
            let mut block_env = BlockEnv::default();
            block_env.number = block_number;

            let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());
            let assertion_contract = get_assertion_selectors(
                Bytecode::LegacyRaw(bytecode(FN_SELECTOR)),
                block_env,
                multi_fork_db,
            )
            .unwrap();

            // Verify the selectors are consistent across different block numbers
            let expected_assertion = selector_assertion();
            assert_eq!(
                assertion_contract.fn_selectors, expected_assertion.fn_selectors,
                "Selectors should be consistent at block {block_number}"
            );
        }
    }
}
