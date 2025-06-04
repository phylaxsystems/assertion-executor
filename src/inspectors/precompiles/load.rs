use crate::{
    db::MultiForkDb,
    inspectors::sol_primitives::PhEvm::loadCall,
    primitives::{
        Address,
        Bytes,
    },
};
use revm::{
    interpreter::CallInputs,
    DatabaseRef,
    InnerEvmContext,
};

use alloy_sol_types::{
    SolCall,
    SolValue,
};
use std::convert::Infallible;

/// Returns a storage slot for a given address. Will return `0x0` if slot empty.
pub fn load_external_slot(
    context: &InnerEvmContext<&mut MultiForkDb<impl DatabaseRef>>,
    call_inputs: &CallInputs,
) -> Result<Bytes, Infallible> {
    let call = match loadCall::abi_decode(&call_inputs.input, true) {
        Ok(call) => call,
        Err(_) => return Ok(Bytes::default()),
    };
    let address: Address = call.target;

    let slot = call.slot;

    let slot_value = match context.db.active_db.storage_ref(address, slot.into()) {
        Ok(rax) => rax,
        Err(_) => return Ok(Bytes::default()),
    };

    Ok(SolValue::abi_encode(&slot_value).into())
}

#[cfg(test)]
mod test {
    use crate::test_utils::run_precompile_test;

    #[tokio::test]
    async fn test_get_storage() {
        let result = run_precompile_test("TestLoad").await;
        assert!(result.is_valid());
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
