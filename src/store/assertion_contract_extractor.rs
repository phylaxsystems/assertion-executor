use std::convert::Infallible;

use crate::{
    build_evm::new_evm,
    db::{
        fork_db::ForkDb,
        multi_fork_db::MultiForkDb,
    },
    executor::CALLER,
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
        ExecutionResult,
        SpecId,
        TxEnv,
        TxKind,
    },
    store::AssertionStoreReader,
    AssertionExecutor,
    ExecutorError,
};

use revm::db::EmptyDB;

use alloy_sol_types::{
    sol,
    Error as SolError,
    SolCall,
};

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
}

/// Extracts [`AssertionContract`] from a given assertion contract's deployment bytecode.
#[derive(Debug)]
pub struct AssertionContractExtractor {
    executor: AssertionExecutor<EmptyDB>,
    empty_multi_fork: MultiForkDb<ForkDb<EmptyDB>>,
}

impl Default for AssertionContractExtractor {
    fn default() -> Self {
        Self::new(SpecId::LATEST, 1)
    }
}

impl AssertionContractExtractor {
    /// Creates a new instance of the extractor with the specified spec id.
    pub fn new(spec_id: SpecId, chain_id: u64) -> Self {
        let empty_db = EmptyDB::default();
        let fork = ForkDb::new(empty_db);
        let empty_multi_fork = MultiForkDb::new(fork.clone(), fork);

        let unused_reader = AssertionStoreReader::new(
            tokio::sync::mpsc::channel(1).0,
            std::time::Duration::default(),
        );

        Self {
            executor: AssertionExecutor {
                db: empty_db,
                spec_id,
                chain_id,
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
        let assertion_address = self.executor.deploy_assertion_contract(
            block_env.clone(),
            assertion_code.clone(),
            &mut multi_fork_db,
            binding,
        )?;

        let inspector = PhEvmInspector::new(self.executor.spec_id, &mut multi_fork_db, binding);

        // Set up and execute the call
        let mut evm = new_evm(
            TxEnv {
                transact_to: TxKind::Call(assertion_address),
                caller: CALLER,
                data: fnSelectorsCall::SELECTOR.into(),
                ..Default::default()
            },
            block_env.clone(),
            self.executor.chain_id,
            self.executor.spec_id,
            &mut multi_fork_db,
            inspector,
        );

        let result = evm.transact().map_err(ExecutorError::TxError)?;

        // Extract and decode selectors from the result
        let fn_selectors = match result.result {
            ExecutionResult::Success { output, .. } => {
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

    let assertion_contract_extractor = AssertionContractExtractor::new(SpecId::LATEST, 1);

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
