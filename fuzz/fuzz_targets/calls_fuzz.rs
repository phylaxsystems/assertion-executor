#![no_main]
use alloy_primitives::FixedBytes;
use alloy_primitives::Log;
use alloy_sol_types::{
    SolCall,
    SolType,
};
use assertion_executor::inspectors::sol_primitives::PhEvm;
use assertion_executor::inspectors::CallTracer;
use assertion_executor::inspectors::PhEvmContext;
use assertion_executor::{
    inspectors::{
        precompiles::calls::get_call_inputs,
        sol_primitives::PhEvm::loadCall,
    },
    primitives::{
        Address,
        Bytes,
        U256,
    },
};
use libfuzzer_sys::fuzz_target;

use revm::interpreter::{
    CallInputs,
    CallScheme,
};

/// Struct that returns generated call input params so we can querry them
/// for validity
#[derive(Debug, Default)]
#[allow(dead_code)]
struct CallInputParams {
    target: Address,
    slot: U256,
}

/// Helper to create CallInputs from fuzzer data
fn create_call_inputs(data: &[u8]) -> (CallInputs, CallInputParams) {
    // Need at least enough bytes for target address + slot
    if data.len() < 52 {
        let tx_env = revm::primitives::TxEnv {
            caller: Address::default(),
            value: U256::ZERO,
            data: Bytes::default(),
            gas_limit: 100_000,
            transact_to: revm::primitives::TxKind::Call(Address::default()),
            ..Default::default()
        };
        return (
            CallInputs::new(&tx_env, 100_000).expect("Unable to create default CallInputs"),
            CallInputParams::default(),
        );
    }

    // Extract target address (20 bytes)
    let mut target_bytes = [0u8; 20];
    target_bytes.copy_from_slice(&data[0..20]);
    let target_address = Address::from(target_bytes);

    // Extract slot (32 bytes)
    let mut slot_bytes = [0u8; 32];
    slot_bytes.copy_from_slice(&data[20..52]);
    let slot = U256::from_be_bytes(slot_bytes);

    // Create a valid loadCall input
    let call = loadCall {
        target: target_address,
        slot: slot.into(),
    };

    let call_input_params = CallInputParams {
        target: target_address,
        slot,
    };

    // Encode the call
    let encoded = loadCall::abi_encode(&call);

    // Extract additional address values if available
    let caller = if data.len() >= 72 {
        let mut caller_bytes = [0u8; 20];
        caller_bytes.copy_from_slice(&data[52..72]);
        Address::from(caller_bytes)
    } else {
        Address::default()
    };

    // Set a proper gas limit based on fuzzer data
    let gas_limit = if data.len() >= 80 {
        let mut gas_bytes = [0u8; 8];
        gas_bytes.copy_from_slice(&data[72..80]);
        u64::from_be_bytes(gas_bytes)
    } else {
        100000 // Default gas limit
    };

    // Extract value if available
    let value = if data.len() >= 112 {
        let mut value_bytes = [0u8; 32];
        value_bytes.copy_from_slice(&data[80..112]);
        U256::from_be_bytes(value_bytes)
    } else {
        U256::ZERO
    };

    let tx_env = revm::primitives::TxEnv {
        caller,
        value,
        data: encoded.into(),
        gas_limit,
        transact_to: revm::primitives::TxKind::Call(target_address),
        ..Default::default()
    };

    (
        CallInputs::new(&tx_env, gas_limit).expect("Unable to create CallInputs"),
        call_input_params,
    )
}

// The fuzz target definition for libFuzzer
fuzz_target!(|data: &[u8]| {
    // Need at least some minimum amount of data
    if data.len() < 65 {
        return;
    }

    // Create call inputs from fuzzer data
    let (call_inputs, params) = create_call_inputs(data);

    // Extract the selector from the call_inputs
    let selector: alloy_primitives::FixedBytes<4> = if call_inputs.input.len() >= 4 {
        FixedBytes::from_slice(&call_inputs.input[..4])
    } else {
        FixedBytes::default()
    };

    // Create a minimally viable context
    let log_array: &[Log] = &[];
    let mut call_tracer = CallTracer::default();

    // Store the original call input in the tracer (WITH the selector)
    call_tracer.record_call(call_inputs.clone());
    let context = PhEvmContext::new(log_array, &call_tracer);

    // Create a precompile call input with the target and selector
    let mut precompile_input = Vec::with_capacity(68);

    // Precompile function selector (4 bytes)
    precompile_input.extend_from_slice(&[0x12, 0x34, 0x56, 0x78]);

    // First parameter: target address (padded to 32 bytes)
    let mut param1 = [0u8; 32];
    param1[12..32].copy_from_slice(params.target.as_slice());
    precompile_input.extend_from_slice(&param1);

    // Second parameter: selector (padded to 32 bytes)
    let mut param2 = [0u8; 32];
    param2[28..32].copy_from_slice(&selector[..]);
    precompile_input.extend_from_slice(&param2);

    let precompile_call = CallInputs {
        input: Bytes::from(precompile_input),
        return_memory_offset: 0..0,
        gas_limit: call_inputs.gas_limit,
        bytecode_address: params.target,
        target_address: params.target,
        caller: call_inputs.caller,
        value: call_inputs.value.clone(),
        scheme: CallScheme::Call,
        is_static: false,
        is_eof: false,
    };

    // Call the target function and catch any panics
    let _ = std::panic::catch_unwind(|| {
        match get_call_inputs(&precompile_call, &context) {
            Ok(rax) => {
                // IMPORTANT: Create the expected encoding with the FULL input
                // (including selector + potentially some prefix bytes)
                let inputs = PhEvm::CallInputs {
                    caller: call_inputs.caller,
                    gas_limit: call_inputs.gas_limit,
                    bytecode_address: call_inputs.target_address,
                    input: call_inputs.input,
                    target_address: call_inputs.target_address,
                    value: call_inputs.value.get(),
                };
                let inputs = vec![inputs];
                let encoded: Bytes =
                    <alloy_sol_types::sol_data::Array<PhEvm::CallInputs>>::abi_encode(&inputs)
                        .into();
                assert_eq!(rax, encoded);
            }
            Err(err) => {
                panic!("Error loading external call inputs: {err}");
            }
        }
    });
});
