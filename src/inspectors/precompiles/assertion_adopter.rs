use crate::{
    inspectors::phevm::PhEvmContext,
    primitives::Bytes,
};

use alloy_sol_types::SolValue;
use std::convert::Infallible;

/// Returns the assertion adopter as a bytes array
pub fn get_assertion_adopter(context: &PhEvmContext) -> Result<Bytes, Infallible> {
    Ok(context.adopter.abi_encode().into())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        inspectors::{
            phevm::{LogsAndTraces, PhEvmContext},
            tracer::CallTracer,
        },
        test_utils::{run_precompile_test, random_address},
    };
    use alloy_primitives::Address;
    use alloy_sol_types::SolValue;

    fn with_adopter_context<F, R>(adopter: Address, f: F) -> R 
    where 
        F: FnOnce(&PhEvmContext) -> R 
    {
        let call_tracer = CallTracer::new();
        let logs_and_traces = LogsAndTraces {
            tx_logs: &[],
            call_traces: &call_tracer,
        };

        let context = PhEvmContext {
            logs_and_traces: &logs_and_traces,
            adopter,
        };
        f(&context)
    }

    #[test]
    fn test_get_assertion_adopter_zero_address() {
        let adopter = Address::ZERO;
        
        let result = with_adopter_context(adopter, |context| {
            get_assertion_adopter(context)
        });
        assert!(result.is_ok());

        let encoded = result.unwrap();
        assert!(!encoded.is_empty());

        // Verify we can decode the result back to the original address
        let decoded = Address::abi_decode(&encoded, false);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), adopter);
    }

    #[test]
    fn test_get_assertion_adopter_random_address() {
        let adopter = random_address();
        
        let result = with_adopter_context(adopter, |context| {
            get_assertion_adopter(context)
        });
        assert!(result.is_ok());

        let encoded = result.unwrap();
        assert!(!encoded.is_empty());

        // Verify we can decode the result back to the original address
        let decoded = Address::abi_decode(&encoded, false);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), adopter);
    }

    #[tokio::test]
    async fn test_get_assertion_adopter_integration() {
        let result = run_precompile_test("TestGetAdopter").await;
        assert!(result.is_valid());
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
