pub mod builder;

use crate::{
    db::{
        fork_db::ForkDb,
        multi_fork_db::MultiForkDb,
        DatabaseCommit,
        DatabaseRef,
        PhDB,
    },
    error::ExecutorError,
    inspectors::{
        phevm::{
            insert_precompile_account,
            PhEvmInspector,
        },
        tracer::CallTracer,
    },
    primitives::{
        address,
        Address,
        AssertionContract,
        AssertionId,
        AssertionResult,
        BlockEnv,
        Bytecode,
        EvmState,
        ExecutionResult,
        FixedBytes,
        ResultAndState,
        TxEnv,
        TxKind,
        U256,
    },
    store::AssertionStoreReader,
};

use alloy_sol_types::{
    sol,
    SolCall,
};

use rayon::prelude::{
    IntoParallelIterator,
    ParallelIterator,
};
use revm::{
    inspector_handle_register,
    primitives::{
        keccak256,
        SpecId,
    },
    Evm,
    EvmBuilder,
};

use tracing::{
    instrument,
    trace,
};

/// Used to deploys the assertion contract to the forked db, and to call assertion functions.
pub const CALLER: Address = address!("00000000000000000000000000000000000001A4");

/// The address of the assertion contract.
/// This is a fixed address that is used to deploy assertion contracts.
/// Deploying assertion contracts via the caller address @ nonce 0 results in this address
pub const ASSERTION_CONTRACT: Address = address!("63f9abbe8aa6ba1261ef3b0cbfb25a5ff8eeed10");

// Typing for the assertion fn selectors
sol! {

    #[derive(Debug)]
    function fnSelectors() external returns (bytes4[] memory);
}

#[derive(Debug, Clone)]
pub struct AssertionExecutor<DB> {
    pub db: DB,
    pub assertion_store_reader: AssertionStoreReader,
    pub spec_id: SpecId,
}

type ExecutorResult<T, DB> = Result<T, ExecutorError<<DB as DatabaseRef>::Error>>;

impl<DB: PhDB> AssertionExecutor<DB>
where
    DB: DatabaseRef<Error: std::fmt::Debug + Send>,
{
    /// Executes a transaction against the current state of the fork, and runs the appropriate
    /// assertions.
    /// Returns the results of the assertions, as well as the state changes that should be
    /// committed if the assertions pass.
    #[instrument(skip_all)]
    pub async fn validate_transaction<'a>(
        &'a mut self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut ForkDb<DB>,
    ) -> ExecutorResult<Option<EvmState>, DB> {
        let pre_tx_db = fork_db.clone();

        let mut post_tx_db = fork_db.clone();

        let (tx_traces, result_and_state) =
            self.execute_forked_tx(block_env.clone(), tx_env, &mut post_tx_db)?;

        let multi_fork_db = MultiForkDb::new(pre_tx_db, post_tx_db);

        let results = self
            .execute_assertions(block_env, tx_traces, multi_fork_db)
            .await?;

        match results.iter().all(|r| r.is_success()) {
            true => {
                fork_db.commit(result_and_state.state.clone());
                Ok(Some(result_and_state.state))
            }
            false => Ok(None),
        }
    }

    #[instrument(skip_all)]
    async fn execute_assertions<'a>(
        &'a self,
        block_env: BlockEnv,
        traces: CallTracer,
        multi_fork_db: MultiForkDb<ForkDb<DB>>,
    ) -> ExecutorResult<Vec<AssertionResult>, DB> {
        // Examine contracts that were called to see if they have assertions
        // associated, and dispatch accordingly
        let mut assertion_store_reader = self.assertion_store_reader.clone();

        let assertions = assertion_store_reader
            .read(block_env.number, traces)
            .await?;

        let mut valid_results = vec![];

        let results = assertions
            .into_par_iter()
            .map(move |assertion_contract| {
                self.run_assertion_contract(
                    &assertion_contract,
                    block_env.clone(),
                    multi_fork_db.clone(),
                )
            })
            .collect::<Vec<ExecutorResult<Vec<AssertionResult>, DB>>>();

        for result in results {
            match result {
                Ok(results) => valid_results.extend(results),
                Err(e) => return Err(e),
            }
        }

        Ok(valid_results)
    }

    fn run_assertion_contract(
        &self,
        assertion_contract: &AssertionContract,
        block_env: BlockEnv,
        mut fork_db: MultiForkDb<ForkDb<DB>>,
    ) -> ExecutorResult<Vec<AssertionResult>, DB> {
        let AssertionContract {
            fn_selectors,
            code,
            code_hash,
        } = assertion_contract;

        trace!(?code_hash, "Running assertion contract");

        //Deploy the assertion contract
        let assertion_address = self
            .deploy_assertion_contract(block_env.clone(), code.clone(), &mut fork_db)
            .expect("Failed to deploy assertion contract");

        //Execute the functions in parallel against cloned forks of the intermediate state, with
        //the deployed assertion contract as the target.
        let results_vec = fn_selectors
            .into_par_iter()
            .map(|fn_selector: &FixedBytes<4>| {
                let mut evm = EvmBuilder::default()
                    .with_db(fork_db.clone())
                    .modify_db(insert_precompile_account)
                    .with_block_env(block_env.clone())
                    .with_spec_id(self.spec_id)
                    .with_external_context(PhEvmInspector::new(self.spec_id))
                    .append_handler_register(inspector_handle_register)
                    .modify_tx_env(|env| {
                        *env = TxEnv {
                            transact_to: TxKind::Call(assertion_address),
                            caller: CALLER,
                            data: (*fn_selector).into(),
                            ..Default::default()
                        }
                    })
                    .build();

                let result = evm
                    .transact()
                    .map(|result_and_state| result_and_state.result)?;

                let id = AssertionId {
                    fn_selector: *fn_selector,
                    code_hash: *code_hash,
                };

                Ok(AssertionResult { id, result })
            })
            .collect::<Vec<ExecutorResult<AssertionResult, DB>>>();

        let mut valid_results = vec![];
        for result in results_vec {
            match result {
                Ok(result) => valid_results.push(result),
                Err(e) => return Err(e),
            }
        }

        Ok(valid_results)
    }

    /// Commits a transaction against a fork of the current state.
    /// Returns the state changes that should be committed if the transaction is valid.
    pub fn execute_forked_tx(
        &self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut ForkDb<DB>,
    ) -> ExecutorResult<(CallTracer, ResultAndState), DB> {
        let mut evm = Evm::builder()
            .with_ref_db(fork_db.clone())
            .with_external_context(CallTracer::default())
            .with_block_env(block_env)
            .with_spec_id(self.spec_id)
            .modify_tx_env(|env| *env = tx_env)
            .append_handler_register(inspector_handle_register)
            .build();

        let result = evm.transact()?;

        fork_db.commit(result.state.clone());

        Ok((evm.context.external, result))
    }

    /// Deploys an assertion contract to the forked db.
    /// Returns the address of the deployed contract.
    fn deploy_assertion_contract(
        &self,
        block_env: BlockEnv,
        constructor_code: Bytecode,
        multi_fork_db: &mut MultiForkDb<ForkDb<DB>>,
    ) -> ExecutorResult<Address, DB> {
        let tx_env = TxEnv {
            transact_to: TxKind::Create,
            caller: CALLER,
            data: constructor_code.original_bytes(),
            ..Default::default()
        };

        let mut evm = Evm::builder()
            .with_db(multi_fork_db.clone())
            .modify_db(insert_precompile_account)
            .with_external_context(PhEvmInspector::new(self.spec_id))
            .with_spec_id(self.spec_id)
            .with_block_env(BlockEnv {
                basefee: U256::ZERO,
                ..block_env
            })
            .modify_tx_env(|env| *env = tx_env)
            .append_handler_register(inspector_handle_register)
            .build();

        let result = evm.transact()?;

        multi_fork_db.commit(result.state.clone());

        Ok(ASSERTION_CONTRACT)
    }

    /// As a part of the spec, assertions must implement the `fnSelectors()` function.
    /// The function returns `bytes4[] memory` of function selectors.
    ///
    /// This function executes the `fnSelectors` function and returns an `AssertionContract`.
    /// If the function is unimplemented, or otherwise errors, it returns `None`.
    pub fn get_assertion_selectors(
        &self,
        block_env: BlockEnv,
        assertion_code: Bytecode,
        mut fork_db: MultiForkDb<ForkDb<DB>>,
    ) -> ExecutorResult<Option<AssertionContract>, DB> {
        // Deploy the contract first
        let assertion_address = self.deploy_assertion_contract(
            block_env.clone(),
            assertion_code.clone(),
            &mut fork_db,
        )?;

        // Set up and execute the call
        let result = EvmBuilder::default()
            .with_db(fork_db)
            .modify_db(insert_precompile_account)
            .with_external_context(PhEvmInspector::new(SpecId::LATEST))
            .with_spec_id(SpecId::LATEST)
            .with_block_env(block_env)
            .modify_tx_env(|env| {
                *env = TxEnv {
                    transact_to: TxKind::Call(assertion_address),
                    caller: CALLER,
                    data: fnSelectorsCall::SELECTOR.into(),
                    ..Default::default()
                }
            })
            .append_handler_register(inspector_handle_register)
            .build()
            .transact()?;

        // Extract and decode selectors from the result
        let selectors = match result.result {
            ExecutionResult::Success { output, .. } => {
                match fnSelectorsCall::abi_decode_returns(output.data(), true) {
                    Ok(selectors) => selectors._0,
                    Err(_) => {
                        return Ok(None);
                    }
                }
            }
            _ => return Ok(None),
        };

        Ok(Some(AssertionContract {
            code_hash: keccak256(assertion_code.original_bytes()),
            code: assertion_code,
            fn_selectors: selectors,
        }))
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::{
        db::{
            DatabaseRef,
            SharedDB,
        },
        primitives::{
            uint,
            Account,
            BlockChanges,
            BlockEnv,
            U256,
        },
        store::AssertionStore,
        test_utils::*,
        AssertionExecutorBuilder,
    };

    use std::collections::HashMap;

    #[tokio::test]
    async fn test_deploy_assertion_contract() {
        let shared_db = SharedDB::<0>::new_test();
        let assertion_store = AssertionStore::default();

        let executor =
            AssertionExecutorBuilder::new(shared_db.clone(), assertion_store.reader()).build();

        let mut multi_fork_db0 = MultiForkDb::new(shared_db.fork(), shared_db.fork());

        let deployed_address0 = executor
            .deploy_assertion_contract(
                BlockEnv::default(),
                Bytecode::LegacyRaw(bytecode(SIMPLE_ASSERTION_COUNTER)),
                &mut multi_fork_db0,
            )
            .unwrap();

        assert_eq!(deployed_address0, ASSERTION_CONTRACT);

        let deployed_code0 = multi_fork_db0
            .basic_ref(deployed_address0)
            .unwrap()
            .unwrap()
            .code
            .unwrap();
        assert!(!deployed_code0.is_empty());

        let mut shortened_code = bytecode(SIMPLE_ASSERTION_COUNTER);
        let len = shortened_code.len();
        shortened_code.truncate(len - 1);

        let mut multi_fork_db1 = MultiForkDb::new(shared_db.fork(), shared_db.fork());

        let deployed_address1 = executor
            .deploy_assertion_contract(
                BlockEnv::default(),
                Bytecode::LegacyRaw(shortened_code),
                &mut multi_fork_db1,
            )
            .unwrap();

        assert_eq!(deployed_address1, ASSERTION_CONTRACT);

        let deployed_code1 = multi_fork_db1
            .basic_ref(deployed_address1)
            .unwrap()
            .unwrap()
            .code
            .unwrap();
        assert!(!deployed_code1.is_empty());

        //Assert that the same address is returned for the differing code
        assert_eq!(deployed_address0, deployed_address1);
        assert_ne!(deployed_code0, deployed_code1);
    }

    #[tokio::test]
    async fn test_execute_forked_tx() {
        let mut shared_db = SharedDB::<0>::new_test();

        let block_changes = BlockChanges {
            state_changes: HashMap::from_iter(vec![(
                COUNTER_ADDRESS,
                Account {
                    info: counter_acct_info(),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };
        let _ = shared_db.commit_block(block_changes);

        let assertion_store = AssertionStore::default();

        let executor =
            AssertionExecutorBuilder::new(shared_db.clone(), assertion_store.reader()).build();

        let mut fork_db = shared_db.fork();

        let (traces, result_and_state) = executor
            .execute_forked_tx(BlockEnv::default(), counter_call(), &mut fork_db)
            .unwrap();

        //Traces should contain the call to the counter contract
        assert_eq!(
            traces.calls.into_iter().collect::<Vec<Address>>(),
            vec![COUNTER_ADDRESS]
        );

        // State changes should contain the counter contract and the caller accounts
        let _accounts = result_and_state.state.keys().cloned().collect::<Vec<_>>();

        //Counter is incremented by 1 for fork
        assert_eq!(
            fork_db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
            Ok(uint!(1_U256))
        );

        //Counter is not incremented in the shared db
        assert_eq!(
            executor.db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
            Ok(U256::ZERO)
        );
    }

    #[tokio::test]
    async fn test_validate_tx() -> Result<(), Box<dyn std::error::Error>> {
        let mut shared_db = SharedDB::<0>::new_test();
        let _ = shared_db.commit_block(BlockChanges {
            state_changes: HashMap::from_iter(
                vec![(
                    COUNTER_ADDRESS,
                    Account {
                        info: counter_acct_info(),
                        ..Default::default()
                    },
                )]
                .into_iter(),
            ),
            ..Default::default()
        });

        let assertion_store = AssertionStore::default();

        assertion_store
            .writer()
            .write(
                U256::ZERO,
                vec![(
                    COUNTER_ADDRESS,
                    vec![Bytecode::LegacyRaw(bytecode(SIMPLE_ASSERTION_COUNTER))],
                )],
            )
            .await
            .unwrap();

        let reader = assertion_store.reader();

        let mut executor = AssertionExecutorBuilder::new(shared_db, reader).build();

        let block_env = BlockEnv {
            number: uint!(1_U256),
            ..Default::default()
        };

        let tx = counter_call();

        let mut fork_db = executor.db.fork();

        for (expected_state_before, expected_state_after, expected_result) in [
            (uint!(0_U256), uint!(1_U256), true),  //Counter is incremented
            (uint!(1_U256), uint!(1_U256), false), //Counter is not incremented as assertion fails
        ] {
            //Assert that the state of the counter contract before committing the changes
            assert_eq!(
                fork_db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
                Ok(expected_state_before),
                "Expected state before: {expected_state_before}",
            );

            let result = executor
                .validate_transaction(block_env.clone(), tx.clone(), &mut fork_db)
                .await
                .unwrap();

            assert_eq!(result.is_some(), expected_result);

            //Assert that the state of the counter contract before committing the changes
            assert_eq!(
                fork_db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
                Ok(expected_state_after),
                "Expected state after: {expected_state_after}",
            );
        }

        //Assert that the assertion contract is not persisted in the database
        assert_eq!(executor.db.basic_ref(ASSERTION_CONTRACT).unwrap(), None);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_assertion_selectors() {
        let shared_db = SharedDB::<0>::new_test();

        let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());

        let executor =
            AssertionExecutorBuilder::new(shared_db.clone(), AssertionStore::default().reader())
                .build();

        // Test with valid assertion contract
        let assertion_contract = executor
            .get_assertion_selectors(
                BlockEnv::default(),
                Bytecode::LegacyRaw(bytecode(FN_SELECTOR)),
                multi_fork_db.clone(),
            )
            .unwrap()
            .unwrap();

        // Verify the contract has the expected selectors from the counter assertion
        let expected_assertion = selector_assertion();
        assert_eq!(
            assertion_contract.fn_selectors,
            expected_assertion.fn_selectors
        );
        assert_eq!(assertion_contract.code_hash, expected_assertion.code_hash);

        // Test with invalid return
        let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());
        let result = executor
            .get_assertion_selectors(
                BlockEnv::default(),
                Bytecode::LegacyRaw(bytecode(BAD_FN_SELECTOR)),
                multi_fork_db,
            )
            .unwrap();

        // Should return None for invalid code
        assert!(result.is_none(), "Should return none");

        // Test with empty code
        let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());
        let result = executor
            .get_assertion_selectors(
                BlockEnv::default(),
                Bytecode::LegacyRaw(vec![].into()),
                multi_fork_db,
            )
            .unwrap();

        // Should return None for empty code
        assert!(result.is_none(), "Should return None for empty code");
    }

    #[tokio::test]
    async fn test_get_assertion_selectors_with_different_block_envs() {
        let shared_db = SharedDB::<0>::new_test();

        let executor =
            AssertionExecutorBuilder::new(shared_db.clone(), AssertionStore::default().reader())
                .build();

        // Test with different block numbers
        for block_number in [0u64, 1u64, 1000u64].map(U256::from) {
            let mut block_env = BlockEnv::default();
            block_env.number = block_number;

            let multi_fork_db = MultiForkDb::new(shared_db.fork(), shared_db.fork());
            let assertion_contract = executor
                .get_assertion_selectors(
                    block_env,
                    Bytecode::LegacyRaw(bytecode(FN_SELECTOR)),
                    multi_fork_db,
                )
                .unwrap()
                .unwrap();

            // Verify the selectors are consistent across different block numbers
            let expected_assertion = selector_assertion();
            assert_eq!(
                assertion_contract.fn_selectors, expected_assertion.fn_selectors,
                "Selectors should be consistent at block {block_number}"
            );
        }
    }
}
