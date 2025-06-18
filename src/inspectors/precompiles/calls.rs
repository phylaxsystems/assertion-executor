use crate::{
    inspectors::phevm::PhEvmContext,
    inspectors::sol_primitives::PhEvm::{
        getCallInputsCall,
        CallInputs as PhEvmCallInputs,
    },
    primitives::Bytes,
};

use revm::interpreter::CallInputs;

use alloy_sol_types::{
    SolCall,
    SolType,
};

#[derive(thiserror::Error, Debug)]
pub enum GetCallInputsError {
    #[error("Failed to decode getCallInputs call: {0:?}")]
    FailedToDecodeGetCallInputsCall(#[from] alloy_sol_types::Error),
}

/// Returns the call inputs of a transaction.
pub fn get_call_inputs(
    inputs: &CallInputs,
    context: &PhEvmContext,
) -> Result<Bytes, GetCallInputsError> {
    let get_call_inputs = getCallInputsCall::abi_decode(&inputs.input, true)?;

    let target = get_call_inputs.target;
    let selector = get_call_inputs.selector;

    let binding = Vec::new();

    let call_inputs = context
        .logs_and_traces
        .call_traces
        .call_inputs
        .get(&(target, selector))
        .unwrap_or(&binding);

    let sol_call_inputs = call_inputs
        .iter()
        .map(|input| {
            PhEvmCallInputs {
                input: input.input.clone(),
                gas_limit: input.gas_limit,
                bytecode_address: input.bytecode_address,
                target_address: input.target_address,
                caller: input.caller,
                value: input.value.get(),
            }
        })
        .collect::<Vec<_>>();

    let encoded: Bytes =
        <alloy_sol_types::sol_data::Array<PhEvmCallInputs>>::abi_encode(&sol_call_inputs).into();

    Ok(encoded)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        inspectors::{
            phevm::{
                LogsAndTraces,
                PhEvmContext,
            },
            tracer::CallTracer,
        },
        test_utils::{
            random_address,
            random_bytes,
            random_selector,
            random_u256,
            run_precompile_test,
        },
    };
    use alloy_primitives::{
        Address,
        Bytes,
        FixedBytes,
        U256,
    };
    use revm::interpreter::{
        CallInputs,
        CallScheme,
        CallValue,
    };

    fn create_call_inputs_with_data(target: Address, selector: FixedBytes<4>) -> CallInputs {
        let get_call_inputs = getCallInputsCall { target, selector };

        let input_data = get_call_inputs.abi_encode();

        CallInputs {
            input: Bytes::from(input_data),
            gas_limit: 1_000_000,
            bytecode_address: Address::ZERO,
            target_address: Address::ZERO,
            caller: Address::ZERO,
            value: CallValue::Transfer(U256::ZERO),
            scheme: CallScheme::Call,
            is_static: false,
            is_eof: false,
            return_memory_offset: 0..0,
        }
    }

    #[test]
    fn test_get_call_inputs_success() {
        let target = random_address();
        let selector = random_selector();

        let call_inputs = create_call_inputs_with_data(target, selector);

        // Set up the context
        let mut call_tracer = CallTracer::new();
        let mock_call_input = CallInputs {
            input: Bytes::from(random_bytes::<32>()),
            gas_limit: 100_000,
            bytecode_address: random_address(),
            target_address: target,
            caller: random_address(),
            value: CallValue::Transfer(random_u256()),
            scheme: CallScheme::Call,
            is_static: false,
            is_eof: false,
            return_memory_offset: 0..0,
        };

        call_tracer
            .call_inputs
            .insert((target, selector), vec![mock_call_input.clone()]);

        let logs_and_traces = LogsAndTraces {
            tx_logs: &[],
            call_traces: &call_tracer,
        };

        let context = PhEvmContext {
            logs_and_traces: &logs_and_traces,
            adopter: Address::ZERO,
        };

        let result = get_call_inputs(&call_inputs, &context);
        assert!(result.is_ok());

        let encoded = result.unwrap();
        assert!(!encoded.is_empty());

        // Verify we can decode the result
        let decoded =
            <alloy_sol_types::sol_data::Array<PhEvmCallInputs>>::abi_decode(&encoded, false);
        assert!(decoded.is_ok());
        let decoded_array = decoded.unwrap();
        assert_eq!(decoded_array.len(), 1);
        assert_eq!(
            decoded_array[0].target_address,
            mock_call_input.target_address
        );
    }

    #[test]
    fn test_get_call_inputs_empty_result() {
        let target = random_address();
        let selector = random_selector();

        let call_inputs = create_call_inputs_with_data(target, selector);

        // Create context with no matching call inputs (different target and selector)
        let call_tracer = CallTracer::new();
        let logs_and_traces = LogsAndTraces {
            tx_logs: &[],
            call_traces: &call_tracer,
        };

        let context = PhEvmContext {
            logs_and_traces: &logs_and_traces,
            adopter: Address::ZERO,
        };

        let result = get_call_inputs(&call_inputs, &context);
        assert!(result.is_ok());

        let encoded = result.unwrap();

        // Should return empty array
        let decoded =
            <alloy_sol_types::sol_data::Array<PhEvmCallInputs>>::abi_decode(&encoded, false);
        assert!(decoded.is_ok());
        let decoded_array = decoded.unwrap();
        assert_eq!(decoded_array.len(), 0);
    }

    #[test]
    fn test_get_call_inputs_invalid_input_length() {
        let target = random_address();
        let selector = random_selector();

        let mut call_inputs = create_call_inputs_with_data(target, selector);
        call_inputs.input = Bytes::from(random_bytes::<32>());

        let call_tracer = CallTracer::new();
        let logs_and_traces = LogsAndTraces {
            tx_logs: &[],
            call_traces: &call_tracer,
        };

        let context = PhEvmContext {
            logs_and_traces: &logs_and_traces,
            adopter: Address::ZERO,
        };

        let result = get_call_inputs(&call_inputs, &context);
        assert!(matches!(
            result,
            Err(GetCallInputsError::FailedToDecodeGetCallInputsCall(_))
        ));
    }

    #[test]
    fn test_get_call_inputs_multiple_results() {
        let target = random_address();
        let selector = random_selector();

        let call_inputs = create_call_inputs_with_data(target, selector);

        let input0 = random_bytes::<32>();
        let input1 = random_bytes::<64>();

        // Set up context with multiple call inputs
        let mut call_tracer = CallTracer::new();
        let mock_call_inputs = vec![
            CallInputs {
                input: Bytes::from([selector.as_slice(), input0.as_slice()].concat()),
                gas_limit: 100_000,
                bytecode_address: random_address(),
                target_address: target,
                caller: random_address(),
                value: CallValue::Transfer(random_u256()),
                scheme: CallScheme::Call,
                is_static: false,
                is_eof: false,
                return_memory_offset: 0..0,
            },
            CallInputs {
                input: Bytes::from([selector.as_slice(), input1.as_slice()].concat()),
                gas_limit: 200_000,
                bytecode_address: random_address(),
                target_address: target,
                caller: random_address(),
                value: CallValue::Transfer(random_u256()),
                scheme: CallScheme::Call,
                is_static: false,
                is_eof: false,
                return_memory_offset: 0..0,
            },
        ];

        for input in mock_call_inputs.iter() {
            call_tracer.record_call(input.clone());
        }

        let logs_and_traces = LogsAndTraces {
            tx_logs: &[],
            call_traces: &call_tracer,
        };

        let context = PhEvmContext {
            logs_and_traces: &logs_and_traces,
            adopter: Address::ZERO,
        };

        let result = get_call_inputs(&call_inputs, &context);
        assert!(result.is_ok());

        let encoded = result.unwrap();
        let decoded =
            <alloy_sol_types::sol_data::Array<PhEvmCallInputs>>::abi_decode(&encoded, true)
                .unwrap();
        assert_eq!(decoded.len(), 2);

        // Verify both results are present
        assert_eq!(
            decoded[0].target_address,
            mock_call_inputs[0].target_address
        );
        assert_eq!(decoded[0].input, Bytes::from(input0));
        assert_eq!(
            decoded[1].target_address,
            mock_call_inputs[1].target_address
        );
        assert_eq!(decoded[1].input, Bytes::from(input1));
    }

    #[tokio::test]
    async fn test_get_call_inputs_integration() {
        let result = run_precompile_test("TestGetCallInputs").await;
        assert!(result.is_valid());
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
