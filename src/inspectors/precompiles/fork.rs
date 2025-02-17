use crate::{
    db::multi_fork_db::{
        ForkError,
        ForkId,
        MultiForkDb,
    },
    inspectors::sol_primitives::Error,
    primitives::{
        Bytes,
        JournaledState,
    },
};

use revm::{
    interpreter::{
        CallOutcome,
        Gas,
        InstructionResult,
        InterpreterResult,
    },
    DatabaseRef,
    EvmContext,
    InnerEvmContext,
};

use alloy_sol_types::SolError;

use std::ops::Range;

pub fn fork_pre_state(
    init_journaled_state: &JournaledState,
    context: &mut EvmContext<&mut MultiForkDb<impl DatabaseRef>>,
    gas: Gas,
    memory_offset: Range<usize>,
) -> CallOutcome {
    let InnerEvmContext {
        ref mut db,
        ref mut journaled_state,
        ..
    } = context.inner;

    let result = db.switch_fork(ForkId::PreTx, journaled_state, init_journaled_state);
    fork_result_to_call_outcome(result, gas, memory_offset)
}

pub fn fork_post_state(
    init_journaled_state: &JournaledState,
    context: &mut EvmContext<&mut MultiForkDb<impl DatabaseRef>>,
    gas: Gas,
    memory_offset: Range<usize>,
) -> CallOutcome {
    let InnerEvmContext {
        ref mut db,
        ref mut journaled_state,
        ..
    } = context.inner;

    let result = db.switch_fork(ForkId::PostTx, journaled_state, init_journaled_state);

    fork_result_to_call_outcome(result, gas, memory_offset)
}

/// Convert a fork result to a call outcome.
/// Uses the default require [`Error`] signature for encoding revert messages.
fn fork_result_to_call_outcome(
    result: Result<(), ForkError>,
    gas: Gas,
    memory_offset: Range<usize>,
) -> CallOutcome {
    match result {
        Ok(()) => {
            CallOutcome {
                result: InterpreterResult {
                    result: InstructionResult::Stop,
                    output: Bytes::default(),
                    gas,
                },
                memory_offset,
            }
        }
        Err(e) => {
            CallOutcome {
                result: InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Error::abi_encode(&Error { _0: e.to_string() }).into(),
                    gas,
                },
                memory_offset,
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::test_utils::run_precompile_test;

    #[tokio::test]
    async fn test_fork_switching() {
        let result = run_precompile_test("TestFork").await;
        assert!(result.is_valid());
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
