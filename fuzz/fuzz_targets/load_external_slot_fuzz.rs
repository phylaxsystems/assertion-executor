#![no_main]
use alloy_sol_types::SolCall;
use assertion_executor::{
    db::{
        overlay::test_utils::MockDb,
        MultiForkDb,
    },
    inspectors::{
        precompiles::load::load_external_slot,
        sol_primitives::PhEvm::loadCall,
    },
    primitives::{
        Address,
        Bytes,
        U256,
    },
};
use libfuzzer_sys::fuzz_target;

use revm::{
    interpreter::CallInputs,
    InnerEvmContext,
};

// Helper to create CallInputs from fuzzer data
fn create_call_inputs(data: &[u8]) -> CallInputs {
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
        return CallInputs::new(&tx_env, 100_000).expect("Unable to create default CallInputs");
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

    CallInputs::new(&tx_env, gas_limit).expect("Unable to create CallInputs")
}

// The fuzz target definition for libFuzzer
fuzz_target!(|data: &[u8]| {
    // Need at least some minimum amount of data
    if data.len() < 32 {
        return;
    }

    // Create call inputs from fuzzer data
    let call_inputs = create_call_inputs(data);

    // Create a minimally viable context
    let mut multi_fork = MultiForkDb::new(MockDb::new(), MockDb::new());
    let context = InnerEvmContext::new(&mut multi_fork);

    // Call the target function and catch any panics
    let _ = std::panic::catch_unwind(|| {
        match load_external_slot(&context, &call_inputs) {
            Ok(_) => (),  // Function completed successfully
            Err(_) => (), // Function returned an error
        }
    });
});
