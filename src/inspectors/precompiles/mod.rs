//! # `precompiles`
//!
//! The `precompiles` mod contains the implementations of all the phevm precompiles.
//! Helper methods used across precompiles can be found here, while the rest of
//! the precompile implementations can be found as follows:
//!
//! - `load`: Loads storage from any account.

use crate::primitives::B256;

use revm::interpreter::{
    CallOutcome,
    Gas,
    InstructionResult,
    InterpreterResult,
};

pub mod load;

// Helper fn to return an empty calloutcome early.
pub fn empty_outcome(gas: Gas) -> CallOutcome {
    CallOutcome {
        result: InterpreterResult {
            result: InstructionResult::Return,
            output: B256::default().into(),
            gas,
        },
        memory_offset: 0..0,
    }
}
