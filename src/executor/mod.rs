pub mod config;

use std::{
    fmt::Display,
    sync::atomic::AtomicU64,
};

use crate::{
    build_evm::new_evm,
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
            PhEvmContext,
            PhEvmInspector,
        },
        tracer::CallTracer,
    },
    primitives::{
        address,
        Address,
        AssertionContract,
        AssertionContractExecution,
        AssertionFunctionExecutionResult,
        AssertionFunctionResult,
        AssertionId,
        BlockEnv,
        Bytecode,
        EVMError,
        FixedBytes,
        ResultAndState,
        TxEnv,
        TxKind,
        TxValidationResult,
    },
    revm::Database,
    store::AssertionStore,
    ExecutorConfig,
};

use rayon::prelude::{
    IntoParallelIterator,
    ParallelIterator,
};

use revm::primitives::ExecutionResult;
use tracing::{
    instrument,
    trace,
};

#[cfg(feature = "optimism")]
use crate::executor::config::create_optimism_fields;

/// Used to deploys the assertion contract to the forked db, and to call assertion functions.
pub const CALLER: Address = address!("00000000000000000000000000000000000001A4");

/// The address of the assertion contract.
/// This is a fixed address that is used to deploy assertion contracts.
/// Deploying assertion contracts via the caller address @ nonce 0 results in this address
pub const ASSERTION_CONTRACT: Address = address!("63f9abbe8aa6ba1261ef3b0cbfb25a5ff8eeed10");

#[derive(Debug, Clone)]
pub struct AssertionExecutor<DB> {
    pub db: DB,
    pub config: ExecutorConfig,
    pub store: AssertionStore,
}

impl<DB: PhDB> AssertionExecutor<DB> {
    /// Creates a new assertion executor.
    pub fn new(db: DB, config: ExecutorConfig, store: AssertionStore) -> Self {
        Self { db, config, store }
    }
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
    pub fn validate_transaction<'a>(
        &'a mut self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut ForkDb<DB>,
    ) -> ExecutorResult<TxValidationResult, DB> {
        let pre_tx_db = fork_db.clone();
        let mut post_tx_db = fork_db.clone();

        trace!(target: "assertion-executor::validation", caller = ?tx_env.caller, transact_to = ?tx_env.transact_to, "Executing forked transaction");
        let (tx_traces, result_and_state) =
            self.execute_forked_tx(block_env.clone(), tx_env, &mut post_tx_db)?;

        if !result_and_state.result.is_success() {
            trace!(target: "assertion-executor::validation", "Transaction execution failed, skipping assertions");
            return Ok(TxValidationResult::new(false, result_and_state, vec![]));
        }

        trace!(target: "assertion-executor::validation", "Transaction succeeded, running assertions");
        let multi_fork_db = MultiForkDb::new(pre_tx_db, post_tx_db);

        let context = PhEvmContext::new(result_and_state.result.logs(), &tx_traces);

        let results = self.execute_assertions(block_env, multi_fork_db, &context)?;

        trace!(
            target: "assertion-executor::validation",
            assertions_ran = results.iter().map(|a| a.total_assertion_funcs_ran).sum::<u64>(),
            gas_used = results.iter().map(|a| a.total_assertion_gas).sum::<u64>(),
            "Completed assertion execution"
        );

        let valid = results
            .iter()
            .all(|a| a.assertion_fns_results.iter().all(|r| r.is_success()));
        if valid {
            fork_db.commit(result_and_state.state.clone());
        }
        Ok(TxValidationResult::new(valid, result_and_state, results))
    }

    /// Executes a transaction against an external revm database, and runs the appropriate
    /// assertions.
    ///
    /// We execute against an external database here to satisfy a requirement within op-talos, where
    /// transactions couldnt be properly commited if they weren't touched by the database beforehand.
    ///
    /// Returns the results of the assertions, as well as the state changes that should be
    /// committed if the assertions pass.
    #[instrument(skip_all)]
    pub fn validate_transaction_ext_db<'validation, ExtDb>(
        &'validation mut self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut ForkDb<DB>,
        external_db: &mut ExtDb,
    ) -> ExecutorResult<TxValidationResult, DB>
    where
        ExtDb: Database,
        ExtDb::Error: Display,
    {
        let pre_tx_db = fork_db.clone();
        let mut post_tx_db = fork_db.clone();

        trace!(target: "assertion-executor::validation", caller = ?tx_env.caller, transact_to = ?tx_env.transact_to, "Executing forked transaction");
        let (tx_traces, result_and_state) =
            self.execute_forked_tx_ext_db(block_env.clone(), tx_env, &mut post_tx_db, external_db)?;

        if !result_and_state.result.is_success() {
            trace!(target: "assertion-executor::validation", "Transaction execution failed, skipping assertions");
            return Ok(TxValidationResult::new(false, result_and_state, vec![]));
        }

        trace!(target: "assertion-executor::validation", "Transaction succeeded, running assertions");
        let multi_fork_db = MultiForkDb::new(pre_tx_db, post_tx_db);

        let context = PhEvmContext::new(result_and_state.result.logs(), &tx_traces);

        let results = self.execute_assertions(block_env, multi_fork_db, &context)?;

        trace!(
            target: "assertion-executor::validation",
            assertions_ran = results.iter().map(|a| a.total_assertion_funcs_ran).sum::<u64>(),
            gas_used = results.iter().map(|a| a.total_assertion_gas).sum::<u64>(),
            "Completed assertion execution"
        );

        let valid = results
            .iter()
            .all(|a| a.assertion_fns_results.iter().all(|r| r.is_success()));
        if valid {
            fork_db.commit(result_and_state.state.clone());
        }
        Ok(TxValidationResult::new(valid, result_and_state, results))
    }

    #[instrument(skip_all)]
    fn execute_assertions<'a>(
        &'a self,
        block_env: BlockEnv,
        multi_fork_db: MultiForkDb<ForkDb<DB>>,
        context: &PhEvmContext<'a>,
    ) -> ExecutorResult<Vec<AssertionContractExecution>, DB> {
        // Examine contracts that were called to see if they have assertions
        // associated, and dispatch accordingly
        let assertions = self.store.read(context.call_traces, block_env.number)?;

        trace!(
            target: "assertion-executor::execute_assertions",
            assertion_count = assertions.len(),
            assertion_ids = ?assertions.iter().map(|a| format!("{:?}", a.code_hash)).collect::<Vec<_>>(),
            "Retrieved Assertion contracts from Assertion store"
        );

        let results: ExecutorResult<Vec<AssertionContractExecution>, DB> = assertions
            .into_par_iter()
            .map(
                move |assertion_contract| -> ExecutorResult<AssertionContractExecution, DB> {
                    self.run_assertion_contract(
                        &assertion_contract,
                        block_env.clone(),
                        multi_fork_db.clone(),
                        context,
                    )
                },
            )
            .collect();
        results
    }

    fn run_assertion_contract(
        &self,
        assertion_contract: &AssertionContract,
        block_env: BlockEnv,
        mut multi_fork_db: MultiForkDb<ForkDb<DB>>,
        context: &PhEvmContext,
    ) -> ExecutorResult<AssertionContractExecution, DB> {
        let AssertionContract {
            fn_selectors,
            code,
            code_hash,
        } = assertion_contract;

        trace!(
            target: "executor::assertion",
            ?code_hash,
            selector_count = fn_selectors.len(),
            selectors = ?fn_selectors.iter().map(|s| format!("{:x?}", s)).collect::<Vec<_>>(),
            "Running assertion contract"
        );

        //Deploy the assertion contract
        let execution_result = self.deploy_assertion_contract(
            block_env.clone(),
            code.clone(),
            &mut multi_fork_db,
            context,
        )?;

        if !execution_result.is_success() {
            trace!(
                target: "executor::assertion",
                ?code_hash,
                gas_used = execution_result.gas_used(),
                "Assertion contract deployment failed"
            );
            let result = fn_selectors
                .iter()
                .map(|fn_selector| {
                    AssertionFunctionResult {
                        id: AssertionId {
                            fn_selector: *fn_selector,
                            code_hash: *code_hash,
                        },
                        result: AssertionFunctionExecutionResult::AssertionContractDeployFailure(
                            execution_result.clone(),
                        ),
                    }
                })
                .collect::<Vec<AssertionFunctionResult>>();

            let rax = AssertionContractExecution {
                assertion_fns_results: result,
                total_assertion_gas: 0,
                total_assertion_funcs_ran: 0,
            };

            trace!(
                target: "executor::assertion",
                ?code_hash,
                total_gas = rax.total_assertion_gas,
                assertions_ran = rax.total_assertion_funcs_ran,
                "Assertion contract deployment failed"
            );

            return Ok(rax);
        }

        let deployment_gas = execution_result.gas_used();
        let remaining_gas = self.config.assertion_gas_limit - deployment_gas;

        let assertion_gas = AtomicU64::new(0);
        let assertions_ran = AtomicU64::new(0);

        //Execute the functions in parallel against cloned forks of the intermediate state, with
        //the deployed assertion contract as the target.
        let results_vec = fn_selectors
            .into_par_iter()
            .map(|fn_selector: &FixedBytes<4>| {
                let mut multi_fork_db = multi_fork_db.clone();

                let inspector =
                    PhEvmInspector::new(self.config.spec_id, &mut multi_fork_db, context);
                let mut evm = new_evm(
                    TxEnv {
                        transact_to: TxKind::Call(ASSERTION_CONTRACT),
                        caller: CALLER,
                        data: (*fn_selector).into(),
                        gas_limit: remaining_gas,
                        #[cfg(feature = "optimism")]
                        optimism: create_optimism_fields(),
                        ..Default::default()
                    },
                    block_env.clone(),
                    self.config.chain_id,
                    self.config.spec_id,
                    &mut multi_fork_db,
                    inspector,
                );

                let result = evm
                    .transact()
                    .map(|result_and_state| result_and_state.result)?;

                let id = AssertionId {
                    fn_selector: *fn_selector,
                    code_hash: *code_hash,
                };

                assertion_gas.fetch_add(result.gas_used(), std::sync::atomic::Ordering::Relaxed);
                assertions_ran.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                Ok(AssertionFunctionResult {
                    id,
                    result: AssertionFunctionExecutionResult::AssertionExecutionResult(result),
                })
            })
            .collect::<Vec<ExecutorResult<AssertionFunctionResult, DB>>>();

        let mut valid_results = vec![];
        for result in results_vec {
            match result {
                Ok(result) => valid_results.push(result),
                Err(e) => return Err(e),
            }
        }

        let rax = AssertionContractExecution {
            assertion_fns_results: valid_results,
            total_assertion_gas: deployment_gas + assertion_gas.into_inner(),
            total_assertion_funcs_ran: assertions_ran.into_inner(),
        };

        trace!(
            target: "executor::assertion",
            ?code_hash,
            total_gas = rax.total_assertion_gas,
            assertions_ran = rax.total_assertion_funcs_ran,
            "Assertion contract execution completed"
        );

        Ok(rax)
    }

    /// Commits a transaction against a fork of the current state.
    /// Returns the state changes that should be committed if the transaction is valid.
    pub fn execute_forked_tx(
        &self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut ForkDb<DB>,
    ) -> ExecutorResult<(CallTracer, ResultAndState), DB> {
        trace!(
            target: "executor::tx",
            caller = ?tx_env.caller,
            transact_to = ?tx_env.transact_to,
            gas_limit = ?tx_env.gas_limit,
            "Executing forked transaction"
        );

        let mut db = revm::db::WrapDatabaseRef(&fork_db);

        let mut evm = new_evm(
            tx_env,
            block_env,
            self.config.chain_id,
            self.config.spec_id,
            &mut db,
            CallTracer::default(),
        );

        let result = evm.transact()?;
        let call_tracer = std::mem::take(&mut evm.context.external);

        std::mem::drop(evm);

        fork_db.commit(result.state.clone());

        trace!(
            target: "executor::tx",
            gas_used = ?result.result.gas_used(),
            success = result.result.is_success(),
            "Completed forked transaction execution"
        );

        Ok((call_tracer, result))
    }

    /// Commits a transaction against a fork of the current state.
    /// Instead of using the fork_db, it uses an external database for refrancing state.
    ///
    /// We execute against an external database here to satisfy a requirement within op-talos, where
    /// transactions couldnt be properly commited if they weren't touched by the database beforehand.
    ///
    /// Returns the state changes that should be committed if the transaction is valid.
    pub fn execute_forked_tx_ext_db<ExtDb>(
        &self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut ForkDb<DB>,
        external_db: &mut ExtDb,
    ) -> ExecutorResult<(CallTracer, ResultAndState), DB>
    where
        ExtDb: Database,
        ExtDb::Error: Display,
    {
        trace!(
            target: "executor::tx",
            caller = ?tx_env.caller,
            transact_to = ?tx_env.transact_to,
            gas_limit = ?tx_env.gas_limit,
            "Executing forked transaction with external db"
        );

        let mut evm = new_evm(
            tx_env,
            block_env,
            self.config.chain_id,
            self.config.spec_id,
            external_db,
            CallTracer::default(),
        );

        let result = match evm.transact() {
            Ok(result) => result,
            Err(err) => {
                match err {
                    EVMError::Database(err) => {
                        // This is for databaseref compatibility
                        return Err(ExecutorError::TxError(EVMError::Custom(err.to_string())));
                    }
                    EVMError::Transaction(err) => {
                        return Err(ExecutorError::TxError(EVMError::Transaction(err)));
                    }
                    EVMError::Header(err) => {
                        return Err(ExecutorError::TxError(EVMError::Header(err)));
                    }
                    EVMError::Precompile(err) => {
                        return Err(ExecutorError::TxError(EVMError::Precompile(err)));
                    }
                    EVMError::Custom(err) => {
                        return Err(ExecutorError::TxError(EVMError::Custom(err)));
                    }
                }
            }
        };

        let call_tracer = std::mem::take(&mut evm.context.external);

        std::mem::drop(evm);

        fork_db.commit(result.state.clone());

        trace!(
            target: "executor::tx",
            gas_used = ?result.result.gas_used(),
            success = result.result.is_success(),
            "Completed forked transaction execution"
        );

        Ok((call_tracer, result))
    }

    /// Deploys an assertion contract to the forked db.
    /// Returns the execution result
    pub fn deploy_assertion_contract(
        &self,
        block_env: BlockEnv,
        constructor_code: Bytecode,
        multi_fork_db: &mut MultiForkDb<ForkDb<DB>>,
        context: &PhEvmContext,
    ) -> ExecutorResult<ExecutionResult, DB> {
        let tx_env = TxEnv {
            transact_to: TxKind::Create,
            caller: CALLER,
            data: constructor_code.original_bytes(),
            gas_limit: self.config.assertion_gas_limit,
            #[cfg(feature = "optimism")]
            optimism: create_optimism_fields(),
            ..Default::default()
        };

        let inspector = PhEvmInspector::new(self.config.spec_id, multi_fork_db, context);

        let result = new_evm(
            tx_env,
            block_env,
            self.config.chain_id,
            self.config.spec_id,
            multi_fork_db,
            inspector,
        )
        .transact()?;

        multi_fork_db.commit(result.state.clone());

        trace!(
            target: "executor::assertion",
            gas_used = ?result.result.gas_used(),
            success = result.result.is_success(),
            "Completed assertion contract deployment"
        );

        Ok(result.result)
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
        store::{
            AssertionState,
            AssertionStore,
        },
        test_utils::*,
    };

    use std::collections::HashMap;

    #[tokio::test]
    async fn test_deploy_assertion_contract() {
        let shared_db = SharedDB::<0>::new_test().await;
        let assertion_store = AssertionStore::new_ephemeral().unwrap();

        let executor = ExecutorConfig::default().build(shared_db.clone(), assertion_store);

        let mut multi_fork_db0 = MultiForkDb::new(shared_db.fork(), shared_db.fork());

        let result = executor
            .deploy_assertion_contract(
                BlockEnv::default(),
                Bytecode::LegacyRaw(bytecode(SIMPLE_ASSERTION_COUNTER)),
                &mut multi_fork_db0,
                &PhEvmContext {
                    tx_logs: &[],
                    call_traces: &CallTracer::default(),
                },
            )
            .unwrap();

        assert!(result.gas_used() != 0);
        assert!(result.is_success());

        let deployed_code0 = multi_fork_db0
            .basic_ref(ASSERTION_CONTRACT)
            .unwrap()
            .unwrap()
            .code
            .unwrap();
        assert!(!deployed_code0.is_empty());

        let mut shortened_code = bytecode(SIMPLE_ASSERTION_COUNTER);
        let len = shortened_code.len();
        shortened_code.truncate(len - 1);

        let mut multi_fork_db1 = MultiForkDb::new(shared_db.fork(), shared_db.fork());

        let result = executor
            .deploy_assertion_contract(
                BlockEnv::default(),
                Bytecode::LegacyRaw(shortened_code),
                &mut multi_fork_db1,
                &PhEvmContext {
                    tx_logs: &[],
                    call_traces: &CallTracer::default(),
                },
            )
            .unwrap();

        let deployed_code1 = multi_fork_db1
            .basic_ref(ASSERTION_CONTRACT)
            .unwrap()
            .unwrap()
            .code
            .unwrap();

        assert!(!deployed_code1.is_empty());

        assert!(result.gas_used() != 0);
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_execute_forked_tx() {
        let mut shared_db = SharedDB::<0>::new_test().await;

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

        shared_db.commit_block(block_changes).unwrap();

        let assertion_store = AssertionStore::new_ephemeral().unwrap();

        let executor = ExecutorConfig::default().build(shared_db.clone(), assertion_store);

        let mut fork_db = shared_db.fork();

        let (traces, result_and_state) = executor
            .execute_forked_tx(BlockEnv::default(), counter_call(), &mut fork_db)
            .unwrap();

        //Traces should contain the call to the counter contract
        assert_eq!(
            traces.calls().into_iter().collect::<Vec<Address>>(),
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
        let mut shared_db = SharedDB::<0>::new_test().await;
        shared_db
            .commit_block(BlockChanges {
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
            })
            .unwrap();

        let assertion_store = AssertionStore::new_ephemeral().unwrap();

        assertion_store.insert(
            COUNTER_ADDRESS,
            AssertionState::new_test(bytecode(SIMPLE_ASSERTION_COUNTER)),
        )?;

        let config = ExecutorConfig::default();
        let mut executor = config.clone().build(shared_db, assertion_store);

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
                .unwrap();

            assert_eq!(result.transaction_valid, expected_result);
            assert!(result.total_assertions_gas() > 0);

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
}
