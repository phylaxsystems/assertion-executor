pub mod builder;

use crate::{
    db::{
        CacheDB,
        DatabaseCommit,
        DatabaseRef,
        PhDB,
    },
    error::ExecutorError,
    primitives::{
        address,
        Address,
        AssertionContract,
        AssertionId,
        AssertionResult,
        BlockEnv,
        Bytecode,
        EVMError,
        FixedBytes,
        StateChanges,
        TxEnv,
        TxKind,
    },
    store::{
        AssertionStoreReader,
        AssertionStoreRequest,
    },
    tracer::CallTracer,
};
use rayon::prelude::{
    IntoParallelIterator,
    ParallelIterator,
};
use revm::{
    inspector_handle_register,
    inspectors::NoOpInspector,
    Evm,
    EvmBuilder,
};

use tokio::sync::mpsc::error::SendError;
use tracing::{
    instrument,
    trace,
};

#[derive(Debug)]
pub struct AssertionExecutor<DB> {
    pub db: DB,
    pub assertion_store_reader: AssertionStoreReader,
}

impl<DB: Clone> Clone for AssertionExecutor<DB> {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            assertion_store_reader: self.assertion_store_reader.clone(),
        }
    }
}

impl<DB: PhDB> AssertionExecutor<DB>
where
    ExecutorError: From<EVMError<<DB as DatabaseRef>::Error>>,
{
    const CALLER: Address = address!("0000000000000000000000000000000000000069");

    const ASSERTION_CONTRACT: Address = address!("cb5ee4f88a1b7107fbc4f8668218cee5ecd3264b");

    /// Executes a transaction against the current state of the fork, and runs the appropriate
    /// assertions.
    /// Returns the results of the assertions, as well as the state changes that should be
    /// committed if the assertions pass.
    #[instrument(skip_all)]
    pub fn validate_transaction<'a>(
        &'a mut self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut CacheDB<DB>,
    ) -> Result<Option<StateChanges>, ExecutorError> {
        let mut temp_db = fork_db.clone();

        let (tx_traces, state_changes) =
            self.execute_forked_tx(block_env.clone(), tx_env, &mut temp_db)?;

        let results = self.execute_assertions(block_env, tx_traces, temp_db.clone())?;

        match results.all(|r| r.is_success()) {
            true => {
                fork_db.commit(state_changes.clone());
                Ok(Some(state_changes))
            }
            false => Ok(None),
        }
    }

    #[instrument(skip_all)]
    fn execute_assertions<'a>(
        &'a self,
        block_env: BlockEnv,
        traces: CallTracer,
        fork_db: CacheDB<DB>,
    ) -> Result<impl ParallelIterator<Item = AssertionResult> + 'a, SendError<AssertionStoreRequest>>
    {
        // Examine contracts that were called to see if they have assertions
        // associated, and dispatch accordingly
        let mut assertion_store_reader = self.assertion_store_reader.clone();

        // TODO: improve channel error handling
        let assertions = assertion_store_reader
            .read_sync(block_env.number, traces)
            .expect("Failed to send request")
            .expect("Failed to receive response, channel empty and closed");

        Ok(assertions
            .into_par_iter()
            .map(move |assertion_contract| {
                self.run_assertion_contract(&assertion_contract, block_env.clone(), fork_db.clone())
            })
            .flatten())
    }

    fn run_assertion_contract(
        &self,
        assertion_contract: &AssertionContract,
        block_env: BlockEnv,
        mut fork_db: CacheDB<DB>,
    ) -> Vec<AssertionResult> {
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
        fn_selectors
            .into_par_iter()
            .map(|fn_selector: &FixedBytes<4>| {
                let mut evm = EvmBuilder::default()
                    .with_ref_db(fork_db.clone())
                    .with_block_env(block_env.clone())
                    .with_external_context(NoOpInspector)
                    .append_handler_register(inspector_handle_register)
                    .modify_tx_env(|env| {
                        *env = TxEnv {
                            transact_to: TxKind::Call(assertion_address),
                            caller: Self::CALLER,
                            data: (*fn_selector).into(),
                            ..Default::default()
                        }
                    })
                    .build();

                let result = evm
                    .transact()
                    .map(|result_and_state| result_and_state.result)
                    .map_err(Into::into);

                let id = AssertionId {
                    fn_selector: *fn_selector,
                    code_hash: *code_hash,
                };

                AssertionResult { id, result }
            })
            .collect()
    }

    /// Commits a transaction against a fork of the current state.
    /// Returns the state changes that should be committed if the transaction is valid.
    fn execute_forked_tx(
        &self,
        block_env: BlockEnv,
        tx_env: TxEnv,
        fork_db: &mut CacheDB<DB>,
    ) -> Result<(CallTracer, StateChanges), ExecutorError> {
        let mut evm = Evm::builder()
            .with_ref_db(fork_db.clone())
            .with_external_context(CallTracer::default())
            .with_block_env(block_env)
            .modify_tx_env(|env| *env = tx_env)
            .append_handler_register(inspector_handle_register)
            .build();

        let result = evm.transact()?;

        fork_db.commit(result.state.clone());

        Ok((evm.context.external, result.state))
    }

    /// Deploys an assertion contract to the forked db.
    /// Returns the address of the deployed contract.
    fn deploy_assertion_contract(
        &self,
        block_env: BlockEnv,
        constructor_code: Bytecode,
        fork_db: &mut CacheDB<DB>,
    ) -> Result<Address, ExecutorError> {
        let tx_env = TxEnv {
            transact_to: TxKind::Create,
            caller: Self::CALLER,
            data: constructor_code.original_bytes(),
            ..Default::default()
        };

        self.execute_forked_tx(block_env, tx_env, fork_db)?;

        Ok(Self::ASSERTION_CONTRACT)
    }
}

#[test]
fn test_deploy_assertion_contract() {
    use crate::{
        db::SharedDB,
        store::AssertionStore,
        test_utils::SIMPLE_ASSERTION_COUNTER_CODE,
        AssertionExecutorBuilder,
    };
    let shared_db = SharedDB::<0>::new_test();
    let assertion_store = AssertionStore::default();

    let executor =
        AssertionExecutorBuilder::new(shared_db.clone(), assertion_store.reader()).build();
    let mut fork_db0 = CacheDB::new(shared_db.clone());

    let deployed_address0 = executor
        .deploy_assertion_contract(
            BlockEnv::default(),
            Bytecode::LegacyRaw(SIMPLE_ASSERTION_COUNTER_CODE),
            &mut fork_db0,
        )
        .unwrap();

    assert_eq!(
        deployed_address0,
        AssertionExecutor::<SharedDB<0>>::ASSERTION_CONTRACT
    );

    let deployed_code0 = fork_db0
        .basic_ref(deployed_address0)
        .unwrap()
        .unwrap()
        .code
        .unwrap();
    assert!(deployed_code0.len() > 0);

    let mut shortened_code = SIMPLE_ASSERTION_COUNTER_CODE;
    shortened_code.truncate(SIMPLE_ASSERTION_COUNTER_CODE.len() - 1);

    let mut fork_db1 = CacheDB::new(shared_db);

    let deployed_address1 = executor
        .deploy_assertion_contract(
            BlockEnv::default(),
            Bytecode::LegacyRaw(shortened_code),
            &mut fork_db1,
        )
        .unwrap();

    assert_eq!(
        deployed_address1,
        AssertionExecutor::<SharedDB<0>>::ASSERTION_CONTRACT
    );

    let deployed_code1 = fork_db1
        .basic_ref(deployed_address1)
        .unwrap()
        .unwrap()
        .code
        .unwrap();
    assert!(deployed_code1.len() > 0);

    //Assert that the same address is returned for the differing code
    assert_eq!(deployed_address0, deployed_address1);
    assert_ne!(deployed_code0, deployed_code1);
}

#[test]
fn test_execute_forked_tx() {
    use crate::{
        db::SharedDB,
        primitives::{
            Account,
            BlockChanges,
            BlockEnv,
            U256,
        },
        store::AssertionStore,
        test_utils::{
            counter_acct_info,
            counter_call,
            COUNTER_ADDRESS,
        },
        AssertionExecutorBuilder,
    };

    use revm::primitives::uint;
    use std::collections::HashMap;

    let mut shared_db = SharedDB::<0>::new_test();

    let block_changes = BlockChanges {
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
    };
    shared_db.commit_block(block_changes);

    let assertion_store = AssertionStore::default();

    let executor =
        AssertionExecutorBuilder::new(shared_db.clone(), assertion_store.reader()).build();

    let mut fork_db = CacheDB::new(shared_db.clone());

    let (traces, state_changes) = executor
        .execute_forked_tx(BlockEnv::default(), counter_call(), &mut fork_db)
        .unwrap();

    //Traces should contain the call to the counter contract
    assert_eq!(
        traces.calls.into_iter().collect::<Vec<Address>>(),
        vec![COUNTER_ADDRESS]
    );

    //State changes should contain the counter contract and the caller accounts
    let mut accounts = state_changes.keys().cloned().collect::<Vec<_>>();
    accounts.sort();

    assert_eq!(accounts, vec![counter_call().caller, COUNTER_ADDRESS]);

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

#[test]
fn test_validate_tx() -> Result<(), Box<dyn std::error::Error>> {
    use super::test_utils::{
        counter_acct_info,
        counter_assertion,
        counter_call,
        COUNTER_ADDRESS,
    };
    use crate::{
        db::SharedDB,
        primitives::{
            Account,
            BlockChanges,
            BlockEnv,
            U256,
        },
        store::AssertionStore,
        AssertionExecutorBuilder,
    };

    use revm::primitives::uint;
    use std::collections::HashMap;

    let mut shared_db = SharedDB::<0>::new_test();
    shared_db.commit_block(BlockChanges {
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
        .write_sync(
            U256::ZERO,
            vec![(COUNTER_ADDRESS, vec![counter_assertion()])],
        )
        .unwrap();

    let mut executor = AssertionExecutorBuilder::new(shared_db, assertion_store.reader()).build();

    let block_env = BlockEnv::default();
    let tx = counter_call();

    let mut fork_db = CacheDB::new(executor.db.clone());

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

        let result = executor.validate_transaction(block_env.clone(), tx.clone(), &mut fork_db)?;

        assert_eq!(result.is_some(), expected_result);

        //Assert that the state of the counter contract before committing the changes
        assert_eq!(
            fork_db.storage_ref(COUNTER_ADDRESS, U256::ZERO),
            Ok(expected_state_after),
            "Expected state after: {expected_state_after}",
        );
    }

    //Assert that the assertion contract is not persisted in the database
    assert_eq!(
        executor
            .db
            .basic_ref(AssertionExecutor::<SharedDB<0>>::ASSERTION_CONTRACT)
            .unwrap(),
        None
    );

    Ok(())
}
