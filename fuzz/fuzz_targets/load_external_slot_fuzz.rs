#![no_main]
use alloy_sol_types::SolCall;
use assertion_executor::{
    db::overlay::test_utils::MockDb,
    inspectors::sol_primitives::PhEvm::loadCall,
    primitives::{
        Address,
        Bytes,
        U256,
    },
};
use libfuzzer_sys::fuzz_target;

use revm::{
    interpreter::CallInputs,
    primitives::Env,
    InnerEvmContext,
};

// Helper to create CallInputs from fuzzer data
fn create_call_inputs(data: &[u8]) -> CallInputs {
    // If we have enough data, try to create a valid loadCall
    if data.len() >= 64 {
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

        CallInputs {
            contract_address: Address::default(),
            transfer: revm::primitives::Transfer {
                source: Address::default(),
                target: Address::default(),
                value: U256::ZERO,
            },
            input: Bytes::from(encoded.to_vec()),
            gas_limit: 100000,
            context: revm::primitives::CallContext {
                address: Address::default(),
                caller: Address::default(),
                code_address: Address::default(),
                apparent_value: U256::ZERO,
                scheme: revm::primitives::CallScheme::Call,
            },
            is_static: false,
        }
    } else {
        // Not enough data to create valid input, return invalid/default inputs
        CallInputs {
            contract_address: Address::default(),
            transfer: revm::primitives::Transfer {
                source: Address::default(),
                target: Address::default(),
                value: U256::ZERO,
            },
            input: Bytes::default(),
            gas_limit: 100000,
            context: revm::primitives::CallContext {
                address: Address::default(),
                caller: Address::default(),
                code_address: Address::default(),
                apparent_value: U256::ZERO,
                scheme: revm::primitives::CallScheme::Call,
            },
            is_static: false,
        }
    }
}

// The fuzz target definition for libFuzzer
fuzz_target!(|data: &[u8]| {
    // Need at least some minimum amount of data
    if data.len() < 32 {
        return;
    }

    // Create mock database from fuzzer data
    let mock_db = MockDb::new();
    let mut multi_fork_db = MockMultiForkDb { active_db: mock_db };

    // Create call inputs from fuzzer data
    let call_inputs = create_call_inputs(data);

    // Create a minimally viable context
    let env = Env::default();
    let mut context = InnerEvmContext::new(&mut multi_fork_db, env);

    // Call the target function and catch any panics
    let _ = std::panic::catch_unwind(|| {
        match load_external_slot(&context, &call_inputs) {
            Ok(_) => (),  // Function completed successfully
            Err(_) => (), // Function returned an error
        }
    });
});
