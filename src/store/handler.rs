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
        request::AssertionStoreRequest,
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

use std::collections::HashMap;
use tokio::sync::mpsc;

// TODO: Support reorgs

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

    /// Polls the request receiver for new requests related to the assertion store.
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

                    let forkb = SharedDB::<0>::new_ephemeral().unwrap();

                    for (addr, assertions) in assertions {
                        let assertions = Vec::from_iter(assertions.into_iter().map(|bytecode| {
                            let multi_fork_db = MultiForkDb::new(forkb.fork(), forkb.fork());
                            get_assertion_selectors(bytecode, BlockEnv::default(), multi_fork_db)
                                .unwrap()
                        }));
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
    use crate::primitives::Bytecode;
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
            SIMPLE_ASSERTION_COUNTER_CODE,
        };
        use revm::primitives::uint;

        let req_tx = setup();

        for block_num in [uint!(0_U256), uint!(1_U256)] {
            let write_req = AssertionStoreRequest::Write {
                block_num,
                assertions: vec![(
                    COUNTER_ADDRESS,
                    vec![Bytecode::LegacyRaw(SIMPLE_ASSERTION_COUNTER_CODE)],
                )],
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
            SIMPLE_ASSERTION_COUNTER_CODE,
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
            assertions: vec![(
                COUNTER_ADDRESS,
                vec![Bytecode::LegacyRaw(SIMPLE_ASSERTION_COUNTER_CODE)],
            )],
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

    #[test]
    fn test_get_assertion_selectors() {
        use crate::{
            db::SharedDB,
            test_utils::{
                selector_assertion,
                BAD_FN_SELECTOR_CODE,
                FN_SELECTOR_CODE,
            },
        };

        let shared_db = SharedDB::<0>::new_test();

        let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());

        // Test with valid assertion contract
        let assertion_contract = get_assertion_selectors(
            Bytecode::LegacyRaw(FN_SELECTOR_CODE),
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
            Bytecode::LegacyRaw(BAD_FN_SELECTOR_CODE),
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
        use crate::{
            db::SharedDB,
            test_utils::{
                selector_assertion,
                FN_SELECTOR_CODE,
            },
        };

        let shared_db = SharedDB::<0>::new_test();

        // Test with different block numbers
        for block_number in [0u64, 1u64, 1000u64].map(U256::from) {
            let mut block_env = BlockEnv::default();
            block_env.number = block_number;

            let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());
            let assertion_contract = get_assertion_selectors(
                Bytecode::LegacyRaw(FN_SELECTOR_CODE),
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
