pub mod builder;

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
        phevm::PhEvmInspector,
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
        FixedBytes,
        ResultAndState,
        TxEnv,
        TxKind,
    },
    store::AssertionStoreReader,
};

use rayon::prelude::{
    IntoParallelIterator,
    ParallelIterator,
};
use revm::primitives::{
    Log,
    SpecId,
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

#[derive(Debug, Clone)]
pub struct AssertionExecutor<DB> {
    pub db: DB,
    pub assertion_store_reader: AssertionStoreReader,
    pub spec_id: SpecId,
    pub chain_id: u64,
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
    ) -> ExecutorResult<Option<ResultAndState>, DB> {
        let pre_tx_db = fork_db.clone();

        let mut post_tx_db = fork_db.clone();

        let (tx_traces, result_and_state) =
            self.execute_forked_tx(block_env.clone(), tx_env, &mut post_tx_db)?;

        if !result_and_state.result.is_success() {
            return Ok(Some(result_and_state));
        }

        let multi_fork_db = MultiForkDb::new(pre_tx_db, post_tx_db);

        let results = self
            .execute_assertions(
                block_env,
                tx_traces,
                multi_fork_db,
                result_and_state.result.logs(),
            )
            .await?;

        match results.iter().all(|r| r.is_success()) {
            true => {
                fork_db.commit(result_and_state.state.clone());
                Ok(Some(result_and_state))
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
        tx_logs: &[Log],
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
                    tx_logs,
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
        mut multi_fork_db: MultiForkDb<ForkDb<DB>>,
        tx_logs: &[Log],
    ) -> ExecutorResult<Vec<AssertionResult>, DB> {
        let AssertionContract {
            fn_selectors,
            code,
            code_hash,
        } = assertion_contract;

        trace!(?code_hash, "Deploying assertion contract");

        //Deploy the assertion contract
        let assertion_address = self.deploy_assertion_contract(
            block_env.clone(),
            code.clone(),
            &mut multi_fork_db,
            tx_logs,
        )?;

        //Execute the functions in parallel against cloned forks of the intermediate state, with
        //the deployed assertion contract as the target.
        let results_vec = fn_selectors
            .into_par_iter()
            .map(|fn_selector: &FixedBytes<4>| {
                let mut multi_fork_db = multi_fork_db.clone();

                let inspector = PhEvmInspector::new(self.spec_id, &mut multi_fork_db, tx_logs);
                let mut evm = new_evm(
                    TxEnv {
                        transact_to: TxKind::Call(assertion_address),
                        caller: CALLER,
                        data: (*fn_selector).into(),
                        gas_limit: block_env.gas_limit.try_into().unwrap_or(u64::MAX),
                        ..Default::default()
                    },
                    block_env.clone(),
                    self.chain_id,
                    self.spec_id,
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
        let mut db = revm::db::WrapDatabaseRef(&fork_db);

        let mut evm = new_evm(
            tx_env,
            block_env,
            self.chain_id,
            self.spec_id,
            &mut db,
            CallTracer::default(),
        );

        let result = evm.transact()?;
        let call_tracer = evm.context.external.clone();
        std::mem::drop(evm);

        fork_db.commit(result.state.clone());

        Ok((call_tracer, result))
    }

    /// Deploys an assertion contract to the forked db.
    /// Returns the address of the deployed contract.
    pub fn deploy_assertion_contract(
        &self,
        block_env: BlockEnv,
        constructor_code: Bytecode,
        multi_fork_db: &mut MultiForkDb<ForkDb<DB>>,
        tx_logs: &[Log],
    ) -> ExecutorResult<Address, DB> {
        let tx_env = TxEnv {
            transact_to: TxKind::Create,
            caller: CALLER,
            data: constructor_code.original_bytes(),
            gas_limit: block_env.gas_limit.try_into().unwrap_or(u64::MAX),
            ..Default::default()
        };

        let inspector = PhEvmInspector::new(self.spec_id, multi_fork_db, tx_logs);

        let result = new_evm(
            tx_env,
            block_env,
            self.chain_id,
            self.spec_id,
            multi_fork_db,
            inspector,
        )
        .transact()?;

        multi_fork_db.commit(result.state.clone());

        Ok(ASSERTION_CONTRACT)
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
        store::MockStore,
        test_utils::*,
        AssertionExecutorBuilder,
    };

    use std::collections::HashMap;

    #[tokio::test]
    async fn test_deploy_assertion_contract() {
        let shared_db = SharedDB::<0>::new_test();

        let executor =
            AssertionExecutorBuilder::new(shared_db.clone(), MockStore::default().reader()).build();

        let mut multi_fork_db0 = MultiForkDb::new(shared_db.fork(), shared_db.fork());

        let deployed_address0 = executor
            .deploy_assertion_contract(
                BlockEnv::default(),
                Bytecode::LegacyRaw(bytecode(SIMPLE_ASSERTION_COUNTER)),
                &mut multi_fork_db0,
                &[],
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
                &[],
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

        let executor =
            AssertionExecutorBuilder::new(shared_db.clone(), MockStore::default().reader()).build();

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

        let mut assertion_store = MockStore::default();

        assertion_store
            .insert(
                COUNTER_ADDRESS,
                vec![Bytecode::LegacyRaw(bytecode(SIMPLE_ASSERTION_COUNTER))],
            )
            .unwrap();

        let mut executor =
            AssertionExecutorBuilder::new(shared_db, assertion_store.reader()).build();

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
}
