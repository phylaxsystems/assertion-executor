use crate::{
    inspectors::phevm::PhEvmContext,
    inspectors::sol_primitives::PhEvm,
    primitives::Bytes,
};

use alloy_sol_types::SolType;
use std::convert::Infallible;

/// Get the log outputs.
pub fn get_logs(context: &PhEvmContext) -> Result<Bytes, Infallible> {
    let sol_logs: Vec<PhEvm::Log> = context
        .logs_and_traces
        .tx_logs
        .iter()
        .map(|log| {
            PhEvm::Log {
                topics: log.topics().to_vec(),
                data: log.data.data.clone(),
                emitter: log.address,
            }
        })
        .collect();

    let encoded: Bytes =
        <alloy_sol_types::sol_data::Array<PhEvm::Log>>::abi_encode(&sol_logs).into();

    Ok(encoded)
}

#[cfg(test)]
mod test {
    use crate::test_utils::run_precompile_test;

    #[tokio::test]
    async fn test_get_logs() {
        let result = run_precompile_test("TestGetLogs").await;
        assert!(result.is_valid());
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
