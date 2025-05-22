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
    use crate::test_utils::run_precompile_test;
    #[tokio::test]
    async fn test_get_assertion_adopter() {
        let result = run_precompile_test("TestGetAdopter").await;
        assert!(result.is_valid());
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
