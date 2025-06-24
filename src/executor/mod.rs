pub mod config;

use std::{
    fmt::{
        Debug,
        Display,
    },
    sync::atomic::AtomicU64,
};

use crate::{
    build_evm::{
        new_phevm,
        new_tx_fork_evm,
    },
    db::{
        fork_db::ForkDb,
        multi_fork_db::MultiForkDb,
        DatabaseCommit,
        DatabaseRef,
        PhDB,
    },
    // Ensure ExecutorError is accessible
    error::ExecutorError,
    inspectors::{
        CallTracer,
        LogsAndTraces,
        PhEvmContext,
        PhEvmInspector,
    },
    primitives::{
        address,
        Account,
        AccountInfo,
        AccountStatus,
        Address,
        AssertionContract,
        AssertionContractExecution,
        AssertionFnId,
        AssertionFunctionExecutionResult,
        AssertionFunctionResult,
        BlockEnv,
        EVMError,
        EvmState,
        FixedBytes,
        ResultAndState,
        TxEnv,
        TxKind,
        TxValidationResult,
        U256,
    },
    store::AssertionStore,
    ExecutorConfig,
};

use revm::Database;

use rayon::prelude::{
    IntoParallelIterator,
    ParallelIterator,
};

use tracing::{
    debug,
    error,
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

// Define the result type using the main DB error
type ExecutorResult<T, DB> = Result<T, ExecutorError<<DB as DatabaseRef>::Error>>;

impl<DB: PhDB> AssertionExecutor<DB>
where
    DB: DatabaseRef<Error: std::fmt::Debug + Send>,
{
    /// Executes a transaction against an external revm database, and runs the appropriate
    /// assertions.
    ///
    /// We execute against an external database here to satisfy a requirement within op-talos, where
    /// transactions couldnt be properly commited if they weren't touched by the database beforehand.
    ///
    /// Returns the results of the assertions, as well as the state changes that should be
    /// committed if the assertions pass.
    #[instrument(skip_all)]
    pub fn validate_transaction_ext_db<'validation, ExtDb, Active>(
        &'validation mut self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut ForkDb<Active>,
        external_db: &mut ExtDb,
    ) -> ExecutorResult<TxValidationResult, DB>
    where
        ExtDb: Database,
        ExtDb::Error: Display,
        Active: DatabaseRef + Sync + Send,
        Active::Error: Debug,
    {
        let pre_tx_db = fork_db.clone();
        let mut post_tx_db = fork_db.clone();

        trace!(target: "assertion-executor::validation", caller = ?tx_env.caller, transact_to = ?tx_env.transact_to, "Executing forked transaction");
        // This call relies on From<EVMError<ExtDb::Error>> for ExecutorError<DB::Error>
        let (tx_traces, result_and_state) =
            self.execute_forked_tx_ext_db(block_env.clone(), tx_env, &mut post_tx_db, external_db).map_err(|e| {
            trace!(target: "assertion-executor::validation", error = %e, "error executing forked tx");
            match e {
                ExecutorError::TxError(EVMError::Transaction(e)) => {
                    ExecutorError::TxError(EVMError::Transaction(e))
                }
                ExecutorError::TxError(EVMError::Header(e)) => {
                    ExecutorError::TxError(EVMError::Header(e))
                }
                ExecutorError::TxError(EVMError::Precompile(e)) => {
                    ExecutorError::TxError(EVMError::Precompile(e))
                }
                ExecutorError::TxError(EVMError::Custom(e)) => {
                    ExecutorError::TxError(EVMError::Custom(e))
                }
                _ => {
                    error!(target: "assertion-executor::validation", error = %e, "Unknown error occurred");
                    ExecutorError::TxError(EVMError::Custom(format!("Error: {e}")))
                }
            }
            })?;

        if !result_and_state.result.is_success() {
            trace!(target: "assertion-executor::validation", "Transaction execution failed, skipping assertions");
            return Ok(TxValidationResult::new(true, result_and_state, vec![]));
        }

        trace!(target: "assertion-executor::validation", "Transaction succeeded, running assertions");
        let multi_fork_db = MultiForkDb::new(pre_tx_db, post_tx_db);

        let logs_and_traces = LogsAndTraces {
            tx_logs: result_and_state.result.logs(),
            call_traces: &tx_traces,
        };

        let results = self.execute_assertions(block_env, multi_fork_db, logs_and_traces)?;

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
    fn execute_assertions<'a, Active>(
        &'a self,
        block_env: BlockEnv,
        multi_fork_db: MultiForkDb<ForkDb<Active>>,
        logs_and_traces: LogsAndTraces<'a>,
    ) -> ExecutorResult<Vec<AssertionContractExecution>, DB>
    where
        Active: DatabaseRef + Sync + Send,
        Active::Error: Debug,
    {
        let assertions = self
            .store
            .read(logs_and_traces.call_traces, block_env.number)?;

        trace!(
            target: "assertion-executor::execute_assertions",
            assertion_count = assertions.len(),
            assertion_ids = ?assertions.iter().map(|a| format!("{:?}", a.assertion_contract.id)).collect::<Vec<_>>(),
            "Retrieved Assertion contracts from Assertion store"
        );
        let results: ExecutorResult<Vec<AssertionContractExecution>, DB> = assertions
            .into_par_iter()
            .map(
                move |assertion_for_execution| -> ExecutorResult<AssertionContractExecution, DB> {
                    let phevm_context =
                        PhEvmContext::new(&logs_and_traces, assertion_for_execution.adopter);

                    self.run_assertion_contract(
                        &assertion_for_execution.assertion_contract,
                        &assertion_for_execution.selectors,
                        block_env.clone(),
                        multi_fork_db.clone(),
                        &phevm_context,
                    )
                },
            )
            .collect();
        trace!(target: "assertion-executor::execute_assertions", results=?results, "Assertion Execution Results");
        results
    }

    fn run_assertion_contract<Active>(
        &self,
        assertion_contract: &AssertionContract,
        fn_selectors: &[FixedBytes<4>],
        block_env: BlockEnv,
        mut multi_fork_db: MultiForkDb<ForkDb<Active>>,
        context: &PhEvmContext,
    ) -> ExecutorResult<AssertionContractExecution, DB>
    where
        Active: DatabaseRef + Sync + Send,
        Active::Error: Debug,
    {
        let AssertionContract { id, .. } = assertion_contract;

        trace!(
            target: "assertion-executor::run_assertion_contract",
            assertion_contract_id = ?id,
            selector_count = fn_selectors.len(),
            selectors = ?fn_selectors.iter().map(|s| format!("{s:x?}")).collect::<Vec<_>>(),
            "Running assertion contract"
        );

        self.insert_assertion_contract(assertion_contract, &mut multi_fork_db);

        let assertion_gas = AtomicU64::new(0);
        let assertions_ran = AtomicU64::new(0);

        let results_vec = fn_selectors
            .into_par_iter()
            .map(
                |fn_selector: &FixedBytes<4>| -> ExecutorResult<AssertionFunctionResult, DB> {
                    let mut multi_fork_db = multi_fork_db.clone();

                    let inspector =
                        PhEvmInspector::new(self.config.spec_id, &mut multi_fork_db, context);

                    let tx_env = TxEnv {
                        transact_to: TxKind::Call(ASSERTION_CONTRACT),
                        caller: CALLER,
                        data: (*fn_selector).into(),
                        gas_limit: self.config.assertion_gas_limit,
                        gas_price: block_env.basefee,
                        #[cfg(feature = "optimism")]
                        optimism: create_optimism_fields(),
                        ..Default::default()
                    };
                    debug!(target: "assertion-executor::assertion", tx_env=?tx_env, "Transaction Environment");
                    let mut evm = new_phevm(
                        tx_env,
                        block_env.clone(),
                        self.config.chain_id,
                        self.config.spec_id,
                        &mut multi_fork_db,
                        inspector,
                    );
                    let result = evm
                        .transact()
                        .map(|result_and_state| result_and_state.result)
                        .map_err(|e| {
                            // Convert the error to the appropriate type
                            ExecutorError::TxError(EVMError::Custom(format!("{e:?}")))
                        })?;
                    debug!(target: "assertion-executor::assertion", result=?result, "Assertion executed");
                    assertion_gas
                        .fetch_add(result.gas_used(), std::sync::atomic::Ordering::Relaxed);
                    assertions_ran.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                    Ok(AssertionFunctionResult {
                        id: AssertionFnId {
                            fn_selector: *fn_selector,
                            assertion_contract_id: *id,
                        },
                        result: AssertionFunctionExecutionResult::AssertionExecutionResult(result),
                    })
                },
            )
            .collect::<Vec<ExecutorResult<AssertionFunctionResult, DB>>>();
        debug!(target: "assertion-executor::assertion", results_vec=?results_vec, "Assertion Execution Results");
        let mut valid_results = vec![];
        for result in results_vec {
            valid_results.push(result?);
        }

        let rax = AssertionContractExecution {
            assertion_fns_results: valid_results,
            total_assertion_gas: assertion_gas.into_inner(),
            total_assertion_funcs_ran: assertions_ran.into_inner(),
        };

        trace!(
            target: "assertion-executor::assertion",
            assertion_contract_id = ?id,
            total_gas = rax.total_assertion_gas,
            assertions_ran = rax.total_assertion_funcs_ran,
            "Assertion contract execution completed"
        );

        Ok(rax)
    }

    /// Commits a transaction against a fork of the current state using an external DB.
    pub fn execute_forked_tx_ext_db<ExtDb, Active>(
        &self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut ForkDb<Active>,
        external_db: &mut ExtDb,
    ) -> ExecutorResult<(CallTracer, ResultAndState), DB>
    where
        ExtDb: Database,
        ExtDb::Error: Display,
        Active: DatabaseRef,
    {
        trace!(
            target: "assertion-executor::tx",
            caller = ?tx_env.caller,
            transact_to = ?tx_env.transact_to,
            gas_limit = ?tx_env.gas_limit,
            "Executing forked transaction with external db"
        );

        let mut evm = new_tx_fork_evm(
            tx_env,
            block_env,
            self.config.chain_id,
            self.config.spec_id,
            external_db,
            CallTracer::default(),
        );

        let result = evm.transact().map_err(|e| {
            trace!(target: "assertion-executor::tx", error = %e, "Transaction execution failed");
            match e {
                EVMError::Transaction(e) => ExecutorError::TxError(EVMError::Transaction(e)),
                EVMError::Header(e) => ExecutorError::TxError(EVMError::Header(e)),
                EVMError::Precompile(e) => ExecutorError::TxError(EVMError::Precompile(e)),
                EVMError::Custom(e) => ExecutorError::TxError(EVMError::Custom(e)),
                _ => {
                    error!(target: "assertion-executor::tx", error = %e, "Unknown error occurred");
                    ExecutorError::TxError(EVMError::Custom(format!("Error: {e}")))
                }
            }
        })?;

        let call_tracer = std::mem::take(&mut evm.context.external);
        std::mem::drop(evm);

        // Commit changes to the ForkDb<Active>
        fork_db.commit(result.state.clone());

        trace!(
            target: "assertion-executor::tx",
            gas_used = ?result.result.gas_used(),
            success = result.result.is_success(),
            "Completed forked transaction execution"
        );

        Ok((call_tracer, result))
    }

    /// Inserts pre-deployed assertion contract inside the multi-fork db.
    pub fn insert_assertion_contract<Active>(
        &self,
        assertion_contract: &AssertionContract,
        multi_fork_db: &mut MultiForkDb<ForkDb<Active>>,
    ) where
        Active: DatabaseRef,
    {
        let AssertionContract {
            deployed_code,
            code_hash,
            storage,
            account_status,
            ..
        } = assertion_contract;

        let account_info = AccountInfo {
            nonce: 1,
            // TODO(Odysseas) Why do we need to set the balance to max?
            balance: U256::MAX,
            code: Some(deployed_code.clone()),
            code_hash: *code_hash,
        };

        let account = Account {
            info: account_info,
            storage: storage.clone(),
            status: *account_status,
        };

        let caller_account = Account {
            info: AccountInfo {
                nonce: 42,
                balance: U256::MAX,
                ..Default::default()
            },
            status: AccountStatus::Touched,
            ..Default::default()
        };

        let mut state = EvmState::default();
        state.insert(ASSERTION_CONTRACT, account);
        state.insert(CALLER, caller_account);
        multi_fork_db.commit(state);
        multi_fork_db
            .active_db
            .storage
            .entry(ASSERTION_CONTRACT)
            .or_default()
            .dont_read_from_inner_db = true;
        multi_fork_db
            .active_db
            .storage
            .entry(CALLER)
            .or_default()
            .dont_read_from_inner_db = true;

        trace!(
            target: "assertion-executor::assertion",
            "Inserted assertion contract into multi fork db"
        );
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::db::overlay::test_utils::MockDb;
    use crate::{
        db::{
            overlay::OverlayDb,
            DatabaseRef,
        },
        primitives::{
            uint,
            BlockEnv,
            U256,
        },
        store::{
            AssertionState,
            AssertionStore,
        },
        test_utils::*,
    };
    use revm::db::CacheDB;
    use revm::db::EmptyDBTyped;
    use std::convert::Infallible;

    // Define a concrete error type for tests if needed, or use Infallible
    type TestDbError = Infallible; // Or a custom test error enum

    impl From<String> for ExecutorError<Infallible> {
        fn from(s: String) -> Self {
            ExecutorError::TxError(EVMError::Custom(s))
        }
    }
    // Define the DB type alias used in tests
    type TestDB = OverlayDb<CacheDB<EmptyDBTyped<TestDbError>>>;
    // Define the Fork DB type alias used in tests
    type TestForkDB = ForkDb<TestDB>;

    #[tokio::test]
    async fn test_deploy_assertion_contract() {
        // Use the TestDB type
        let test_db: TestDB = OverlayDb::<CacheDB<EmptyDBTyped<TestDbError>>>::new_test();

        let assertion_store = AssertionStore::new_ephemeral().unwrap();

        // Build uses TestDB
        let executor = ExecutorConfig::default().build(test_db.clone(), assertion_store);

        // Forks use TestDB
        let mut multi_fork_db = MultiForkDb::new(test_db.fork(), test_db.fork());

        let counter_assertion = counter_assertion();

        // insert_assertion_contract is generic, works with MultiForkDb<ForkDb<TestDB>>
        executor.insert_assertion_contract(&counter_assertion, &mut multi_fork_db);

        let account_info = multi_fork_db
            .basic_ref(ASSERTION_CONTRACT)
            .unwrap()
            .unwrap();

        assert_eq!(account_info.code.unwrap(), counter_assertion.deployed_code);
    }

    #[tokio::test]
    async fn test_execute_forked_tx() {
        // Use the TestDB type
        let shared_db: TestDB = OverlayDb::<CacheDB<EmptyDBTyped<TestDbError>>>::new_test();

        let mut mock_db = MockDb::new();
        mock_db.insert_account(COUNTER_ADDRESS, counter_acct_info());

        let assertion_store = AssertionStore::new_ephemeral().unwrap();

        // Build uses TestDB
        let executor = ExecutorConfig::default().build(shared_db.clone(), assertion_store);

        // Fork uses TestDB
        let mut fork_db: TestForkDB = shared_db.fork();

        // execute_forked_tx uses &mut ForkDb<TestDB>
        let (traces, result_and_state) = executor
            .execute_forked_tx_ext_db(
                BlockEnv::default(),
                counter_call(),
                &mut fork_db,
                &mut mock_db,
            )
            .unwrap();

        //Traces should contain the call to the counter contract
        assert_eq!(
            traces.calls().into_iter().collect::<Vec<Address>>(),
            vec![COUNTER_ADDRESS]
        );

        // State changes should contain the counter contract and the caller accounts
        let _accounts = result_and_state.state.keys().cloned().collect::<Vec<_>>();

        // Check storage on the TestForkDB
        assert_eq!(
            fork_db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
            Ok(uint!(1_U256))
        );

        // Check storage on the original TestDB via executor.db
        assert_eq!(
            executor.db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
            Ok(U256::ZERO)
        );
    }
    #[tokio::test]
    async fn test_validate_tx() -> Result<(), Box<dyn std::error::Error>> {
        // Use the TestDB type
        let test_db: TestDB = OverlayDb::<CacheDB<EmptyDBTyped<TestDbError>>>::new_test();

        let mut mock_db = MockDb::new();

        mock_db.insert_account(COUNTER_ADDRESS, counter_acct_info());

        let assertion_store = AssertionStore::new_ephemeral().unwrap();

        // Insert requires Bytes, use helper from test_utils
        let assertion_bytecode = bytecode(SIMPLE_ASSERTION_COUNTER);
        assertion_store.insert(
            COUNTER_ADDRESS,
            // Assuming AssertionState::new_test takes Bytes or similar
            AssertionState::new_test(assertion_bytecode),
        )?;

        let config = ExecutorConfig::default();

        // Build uses TestDB
        let mut executor = config.clone().build(test_db, assertion_store);

        let basefee = uint!(10_U256);
        let block_env = BlockEnv {
            number: uint!(1_U256),
            basefee,
            ..Default::default()
        };

        let tx = TxEnv {
            gas_price: basefee,
            ..counter_call()
        };

        mock_db.insert_account(
            tx.caller,
            AccountInfo {
                balance: U256::MAX,
                ..Default::default()
            },
        );

        // Fork uses TestDB
        let mut fork_db: TestForkDB = executor.db.fork();

        for (expected_state_before, expected_state_after, expected_result) in [
            (uint!(0_U256), uint!(1_U256), true),  // Counter is incremented
            (uint!(1_U256), uint!(1_U256), false), // Counter is not incremented as assertion fails
        ] {
            // Check storage on TestForkDB
            assert_eq!(
                fork_db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
                Ok(expected_state_before),
                "Expected state before: {expected_state_before}",
            );

            // validate_transaction uses &mut ForkDb<TestDB>
            let result = executor
                .validate_transaction_ext_db::<_, _>(
                    block_env.clone(),
                    tx.clone(),
                    &mut fork_db,
                    &mut mock_db,
                )
                .unwrap(); // Use unwrap or handle ExecutorError<TestDbError>

            assert_eq!(result.transaction_valid, expected_result);
            // Only assert gas if the transaction was meant to run assertions
            if result.transaction_valid || !expected_result {
                // If tx valid, or if tx invalid and we expected it to be invalid
                assert!(
                    result.total_assertions_gas() > 0,
                    "Assertions should have run gas"
                );
            }

            if result.transaction_valid {
                mock_db.commit(result.result_and_state.state.clone());
            }

            // Check storage on TestForkDB after potential commit
            assert_eq!(
                fork_db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
                Ok(expected_state_after),
                "Expected state after: {expected_state_after}",
            );
        }

        // Check original TestDB via executor.db
        assert_eq!(executor.db.basic_ref(ASSERTION_CONTRACT).unwrap(), None);

        Ok(())
    }
}
