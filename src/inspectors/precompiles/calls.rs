use crate::{
    inspectors::phevm::PhEvmContext,
    inspectors::sol_primitives::PhEvm,
    primitives::{
        Address,
        Bytes,
        FixedBytes,
    },
};

use revm::interpreter::CallInputs;

use alloy_sol_types::SolType;

#[derive(thiserror::Error, Debug)]
pub enum GetCallInputsError {
    #[error("Invalid input length, less than 64 bytes: {0}")]
    InvalidInputLength(usize),
}

/// Returns the call inputs of a transaction.
pub fn get_call_inputs(
    inputs: &CallInputs,
    context: &PhEvmContext,
) -> Result<Bytes, GetCallInputsError> {
    // Skip function selector (4 bytes)
    let input_data = &inputs.input[4..];

    // Input must be at least 64 bytes (2 * 32 byte parameters)
    if input_data.len() < 64 {
        return Err(GetCallInputsError::InvalidInputLength(input_data.len()));
    }

    // Extract address from first parameter (skip 12 bytes padding)
    let target = Address::from_slice(&input_data[12..32]);

    // Extract selector from second parameter (skip 28 bytes padding)
    let selector = FixedBytes::from_slice(&input_data[32..36]);
    let binding = Vec::new();
    // panic!("context: {:#?}\ntarget: {:?}\nselector {:?}", context, target, selector);
    let call_inputs = context
        .logs_and_traces
        .call_traces
        .call_inputs
        .get(&(target, selector))
        .unwrap_or(&binding);

    let sol_call_inputs: Vec<PhEvm::CallInputs> = call_inputs
        .iter()
        .map(|input| {
            PhEvm::CallInputs {
                input: input.input.clone(),
                gas_limit: input.gas_limit,
                bytecode_address: input.bytecode_address,
                target_address: input.target_address,
                caller: input.caller,
                value: input.value.get(),
            }
        })
        .collect();

    let encoded: Bytes =
        <alloy_sol_types::sol_data::Array<PhEvm::CallInputs>>::abi_encode(&sol_call_inputs).into();

    Ok(encoded)
}

#[cfg(test)]
mod test {
    use crate::test_utils::run_precompile_test;

    #[tokio::test]
    async fn test_get_call_inputs() {
        let result = run_precompile_test("TestGetCallInputs").await;
        assert!(result.is_valid());
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
