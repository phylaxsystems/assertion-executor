use std::convert::Infallible;

use crate::{
    build_evm::new_evm,
    db::DatabaseCommit,
    executor::{
        ASSERTION_CONTRACT,
        CALLER,
    },
    inspectors::{
        insert_trigger_recorder_account,
        TriggerRecorder,
    },
    primitives::{
        keccak256,
        Account,
        AssertionContract,
        BlockEnv,
        Bytes,
        EVMError,
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
    SolCall,
};

#[cfg(feature = "optimism")]
use crate::executor::config::create_optimism_fields;

// Typing for the assertion fn selectors
sol! {
    #[derive(Debug)]
    function triggers() external view;
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FnSelectorExtractorError {
    #[error("Failed to call triggers function")]
    TriggersCallError(EVMError<Infallible>),
    #[error("Failed to call triggers function")]
    TriggersCallFailed,
    #[error("Error with assertion contract deployment")]
    AssertionContractDeployError(EVMError<Infallible>),
    #[error("Assertion contract deployment failed")]
    AssertionContractDeployFailed,
    #[error("No triggers found in assertion contract")]
    NoTriggersFound,
}

/// Extracts [`AssertionContract`] and [`TriggerRecorder`] from a given assertion contract's deployment bytecode
pub fn extract_assertion_contract(
    assertion_code: Bytes,
    config: &ExecutorConfig,
) -> Result<(AssertionContract, TriggerRecorder), FnSelectorExtractorError> {
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

    let init_account = state
        .get(&ASSERTION_CONTRACT)
        .cloned()
        .ok_or(FnSelectorExtractorError::AssertionContractDeployFailed)?;

    db.commit(state);

    insert_trigger_recorder_account(&mut db);

    // Set up and execute the call
    let mut evm = new_evm(
        TxEnv {
            transact_to: TxKind::Call(ASSERTION_CONTRACT),
            caller: CALLER,
            data: triggersCall::SELECTOR.into(),
            gas_limit: config.assertion_gas_limit - result.gas_used(),
            #[cfg(feature = "optimism")]
            optimism: create_optimism_fields(),
            ..Default::default()
        },
        block_env,
        config.chain_id,
        config.spec_id,
        &mut db,
        TriggerRecorder::default(),
    );

    if !evm
        .transact()
        .map_err(FnSelectorExtractorError::TriggersCallError)?
        .result
        .is_success()
    {
        return Err(FnSelectorExtractorError::TriggersCallFailed);
    }

    let Account {
        info,
        storage,
        status,
        ..
    } = init_account;

    let deployed_code = info
        .code
        .ok_or(FnSelectorExtractorError::AssertionContractDeployFailed)?;

    Ok((
        AssertionContract {
            deployed_code,
            code_hash: info.code_hash,
            storage,
            account_status: status,
            id: keccak256(&assertion_code),
        },
        evm.context.external,
    ))
}

#[test]
fn test_get_assertion_selectors() {
    use crate::test_utils::*;

    use crate::primitives::fixed_bytes;

    let config = ExecutorConfig::default();
    // Test with valid assertion contract
    let (_, trigger_recorder) = extract_assertion_contract(bytecode(FN_SELECTOR), &config).unwrap();

    // Verify the contract has the expected selectors from the counter assertion
    let mut expected_selectors = vec![
        fixed_bytes!("e7f48038"),
        fixed_bytes!("1ff1bc3a"),
        fixed_bytes!("d210b7cf"),
    ];
    expected_selectors.sort();

    let mut recorded_selectors = trigger_recorder
        .triggers
        .values()
        .flat_map(|v| v.iter())
        .cloned()
        .collect::<Vec<_>>();
    recorded_selectors.sort();
    assert_eq!(recorded_selectors, expected_selectors);
}
