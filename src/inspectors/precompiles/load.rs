use crate::{
    db::MultiForkDb,
    inspectors::precompiles::empty_outcome,
    inspectors::sol_primitives::PhEvm::loadCall,
    primitives::Address,
    revm::DatabaseRef,
};

use revm::{
    interpreter::{
        CallInputs,
        CallOutcome,
        Gas,
        InstructionResult,
        InterpreterResult,
    },
    InnerEvmContext,
};

use alloy_sol_types::{
    SolCall,
    SolValue,
};

/// Returns a storage slot for a given address. Will return `0x0` if slot empty.
pub fn load_external_slot(
    context: &InnerEvmContext<&mut MultiForkDb<impl DatabaseRef>>,
    call_inputs: &CallInputs,
    gas: Gas,
) -> CallOutcome {
    let call = match loadCall::abi_decode(&call_inputs.input, true) {
        Ok(call) => call,
        Err(_) => return empty_outcome(gas),
    };
    let address: Address = call.target;

    let slot = call.slot;

    let slot_value = match context.db.active_db.storage_ref(address, slot.into()) {
        Ok(rax) => rax,
        Err(_) => return empty_outcome(gas),
    };

    let value = SolValue::abi_encode(&slot_value);

    CallOutcome {
        result: InterpreterResult {
            result: InstructionResult::Return,
            output: value.into(),
            gas,
        },
        memory_offset: call_inputs.return_memory_offset.clone(),
    }
}

#[cfg(test)]
mod test {
    use crate::test_utils::run_precompile_test;

    #[tokio::test]
    async fn test_get_storage() {
        let result = run_precompile_test("TestLoad").await;
        assert!(result.is_some());
        let result_and_state = result.unwrap();
        assert!(result_and_state.result.is_success());
    }
}
