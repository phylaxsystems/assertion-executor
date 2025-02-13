use std::convert::Infallible;

use crate::{
    build_evm::new_evm,
    db::{
        fork_db::ForkDb,
        multi_fork_db::MultiForkDb,
    },
    executor::{
        ASSERTION_CONTRACT,
        CALLER,
    },
    inspectors::{
        phevm::{
            PhEvmContext,
            PhEvmInspector,
        },
        tracer::CallTracer,
    },
    primitives::{
        keccak256,
        AssertionContract,
        BlockEnv,
        Bytecode,
        EvmExecutionResult,
        TxEnv,
        TxKind,
    },
    store::AssertionStoreReader,
    AssertionExecutor,
    ExecutorConfig,
    ExecutorError,
};

use revm::db::EmptyDB;

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
    #[error("Executor error in fn selector extraction")]
    ExecutorError(#[from] ExecutorError<Infallible>),
    #[error("Failed to call fnSelectors function")]
    FnSelectorCallFailed,
    #[error("Failed to deploy assertion contract")]
    AssertionContractDeployError,
}

/// Extracts [`AssertionContract`] from a given assertion contract's deployment bytecode.
#[derive(Debug)]
pub struct AssertionContractExtractor {
    executor: AssertionExecutor<EmptyDB>,
    empty_multi_fork: MultiForkDb<ForkDb<EmptyDB>>,
}

impl Default for AssertionContractExtractor {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl AssertionContractExtractor {
    /// Creates a new instance of the extractor with the specified spec id.
    pub fn new(config: ExecutorConfig) -> Self {
        let empty_db = EmptyDB::default();
        let fork = ForkDb::new(empty_db);
        let empty_multi_fork = MultiForkDb::new(fork.clone(), fork);

        let unused_reader = AssertionStoreReader::new(tokio::sync::mpsc::channel(1).0);

        Self {
            executor: AssertionExecutor {
                db: empty_db,
                config,
                assertion_store_reader: unused_reader,
            },
            empty_multi_fork,
        }
    }

    /// Extracts the assertion contract from the given assertion code.
    pub fn extract_assertion_contract(
        &self,
        assertion_code: Bytecode,
    ) -> Result<AssertionContract, FnSelectorExtractorError> {
        let block_env = BlockEnv::default();

        let mut multi_fork_db = self.empty_multi_fork.clone();

        let binding = &PhEvmContext {
            tx_logs: &[],
            call_traces: &CallTracer::default(),
        };

        // Deploy the contract first
        let result = self.executor.deploy_assertion_contract(
            block_env.clone(),
            assertion_code.clone(),
            &mut multi_fork_db,
            binding,
        )?;

        if !result.is_success() {
            return Err(FnSelectorExtractorError::AssertionContractDeployError);
        }

        let inspector =
            PhEvmInspector::new(self.executor.config.spec_id, &mut multi_fork_db, binding);

        // Set up and execute the call
        let mut evm = new_evm(
            TxEnv {
                transact_to: TxKind::Call(ASSERTION_CONTRACT),
                caller: CALLER,
                data: fnSelectorsCall::SELECTOR.into(),
                gas_limit: self.executor.config.assertion_gas_limit - result.gas_used(),
                #[cfg(feature = "optimism")]
                optimism: create_optimism_fields(),
                ..Default::default()
            },
            block_env.clone(),
            self.executor.config.chain_id,
            self.executor.config.spec_id,
            &mut multi_fork_db,
            inspector,
            true,
        );

        let result = evm.transact().map_err(ExecutorError::TxError)?;

        // Extract and decode selectors from the result
        let fn_selectors = match result.result {
            EvmExecutionResult::Success { output, .. } => {
                fnSelectorsCall::abi_decode_returns(output.data(), true)?._0
            }
            _ => return Err(FnSelectorExtractorError::FnSelectorCallFailed),
        };

        Ok(AssertionContract {
            code_hash: keccak256(assertion_code.original_bytes()),
            code: assertion_code,
            fn_selectors,
        })
    }
}

#[test]
fn test_get_assertion_selectors() {
    use crate::test_utils::*;

    let assertion_contract_extractor = AssertionContractExtractor::default();

    // Test with valid assertion contract
    let assertion_contract = assertion_contract_extractor
        .extract_assertion_contract(Bytecode::LegacyRaw(bytecode(FN_SELECTOR)))
        .unwrap();

    // Verify the contract has the expected selectors from the counter assertion
    let expected_assertion = selector_assertion();
    assert_eq!(
        assertion_contract.fn_selectors,
        expected_assertion.fn_selectors
    );
    assert_eq!(assertion_contract.code_hash, expected_assertion.code_hash);

    // Test with invalid return
    let result = assertion_contract_extractor
        .extract_assertion_contract(Bytecode::LegacyRaw(bytecode(BAD_FN_SELECTOR)));

    // Should return None for invalid code
    assert_eq!(
        result,
        Err(FnSelectorExtractorError::DecodeError(
            SolError::ReserMismatch
        ))
    );

    // Test with empty code
    let result =
        assertion_contract_extractor.extract_assertion_contract(Bytecode::LegacyRaw(vec![].into()));

    // Should return None for empty code
    assert_eq!(
        result,
        Err(FnSelectorExtractorError::DecodeError(SolError::Overrun))
    );
}
