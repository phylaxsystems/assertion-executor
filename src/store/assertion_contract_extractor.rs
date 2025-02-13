use std::convert::Infallible;

use crate::{
    build_evm::new_evm,
    db::DatabaseCommit,
    executor::{
        ASSERTION_CONTRACT,
        CALLER,
    },
    primitives::{
        keccak256,
        AssertionContract,
        BlockEnv,
        Bytecode,
        Bytes,
        EVMError,
        EvmExecutionResult,
        ResultAndState,
        TxEnv,
        TxKind,
    },
    ExecutorConfig,
};

use revm::{
    db::InMemoryDB,
    inspectors::NoOpInspector,
};

use alloy_sol_types::{
    sol,
    Error as SolError,
    SolCall,
};

#[cfg(feature = "optimism")]
use crate::executor::config::create_optimism_fields;

// Typing for the assertion fn selectors
sol! {
    #[derive(Debug)]
    function fnSelectors() external returns (bytes4[] memory);
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FnSelectorExtractorError {
    #[error("Failed to decode fn selectors from contract")]
    DecodeError(#[from] SolError),
    #[error("Failed to call fnSelectors function")]
    FnSelectorCallError(EVMError<Infallible>),
    #[error("Error with assertion contract deployment")]
    AssertionContractDeployError(EVMError<Infallible>),
    #[error("Assertion contract deployment failed")]
    AssertionContractDeployFailed,
    #[error("Failed to call fnSelectors function")]
    FnSelectorCallFailed,
}

/// Extracts [`AssertionContract`] from a given assertion contract's deployment bytecode.
pub fn extract_assertion_contract(
    assertion_code: Bytes,
    config: &ExecutorConfig,
) -> Result<AssertionContract, FnSelectorExtractorError> {
    let block_env = BlockEnv::default();

    // Deploy the contract first
    let tx_env = TxEnv {
        transact_to: TxKind::Create,
        caller: CALLER,
        data: assertion_code.clone(),
        gas_limit: config.assertion_gas_limit,
        #[cfg(feature = "optimism")]
        optimism: create_optimism_fields(),
        ..Default::default()
    };

    let mut db = InMemoryDB::default();

    let ResultAndState { result, state } = new_evm(
        tx_env,
        block_env.clone(),
        config.chain_id,
        config.spec_id,
        &mut db,
        NoOpInspector,
    )
    .transact()
    .map_err(FnSelectorExtractorError::AssertionContractDeployError)?;

    if !result.is_success() {
        return Err(FnSelectorExtractorError::AssertionContractDeployFailed);
    }

    db.commit(state);

    // Set up and execute the call
    let mut evm = new_evm(
        TxEnv {
            transact_to: TxKind::Call(ASSERTION_CONTRACT),
            caller: CALLER,
            data: fnSelectorsCall::SELECTOR.into(),
            gas_limit: config.assertion_gas_limit - result.gas_used(),
            #[cfg(feature = "optimism")]
            optimism: create_optimism_fields(),
            ..Default::default()
        },
        block_env,
        config.chain_id,
        config.spec_id,
        &mut db,
        NoOpInspector,
    );

    let result = evm
        .transact()
        .map_err(FnSelectorExtractorError::FnSelectorCallError)?;

    // Extract and decode selectors from the result
    let fn_selectors = match result.result {
        EvmExecutionResult::Success { output, .. } => {
            fnSelectorsCall::abi_decode_returns(output.data(), true)?._0
        }
        _ => return Err(FnSelectorExtractorError::FnSelectorCallFailed),
    };

    Ok(AssertionContract {
        code_hash: keccak256(&assertion_code),
        code: Bytecode::LegacyRaw(assertion_code),
        fn_selectors,
    })
}

#[test]
fn test_get_assertion_selectors() {
    use crate::test_utils::*;

    let config = ExecutorConfig::default();
    // Test with valid assertion contract
    let assertion_contract = extract_assertion_contract(bytecode(FN_SELECTOR), &config).unwrap();

    // Verify the contract has the expected selectors from the counter assertion
    let expected_assertion = selector_assertion();
    assert_eq!(
        assertion_contract.fn_selectors,
        expected_assertion.fn_selectors
    );
    assert_eq!(assertion_contract.code_hash, expected_assertion.code_hash);

    // Test with invalid return
    let result = extract_assertion_contract(bytecode(BAD_FN_SELECTOR), &config);

    // Should return None for invalid code
    assert_eq!(
        result,
        Err(FnSelectorExtractorError::DecodeError(
            SolError::ReserMismatch
        ))
    );

    // Test with empty code
    let result = extract_assertion_contract(Bytes::new(), &config);

    // Should return None for empty code
    assert_eq!(
        result,
        Err(FnSelectorExtractorError::DecodeError(SolError::Overrun))
    );
}
