pub mod builder;

use crate::{
    db::Ext,
    error::ExecutorError,
    primitives::{
        address,
        AccountInfo,
        Address,
        AssertionContract,
        AssertionId,
        AssertionResult,
        BlockEnv,
        EVMError,
        FixedBytes,
        TxEnv,
        TxKind,
        U256,
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
    Database,
    DatabaseCommit,
    Evm,
    EvmBuilder,
};

use std::marker::{
    Send,
    Sync,
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

impl<DB: Database + DatabaseCommit + Clone + Ext<DB> + std::fmt::Debug + Sync + Send>
    AssertionExecutor<DB>
where
    ExecutorError: From<EVMError<<DB as Database>::Error>>,
{
    const ASSERTION_CONTRACT_ADDRESS: Address =
        address!("0000000000000000000000000000000000000069");

    #[instrument(skip_all)]
    pub fn execute_assertions<'a>(
        &'a mut self,
        block_env: BlockEnv,
        traces: CallTracer,
    ) -> Result<impl ParallelIterator<Item = AssertionResult> + 'a, SendError<AssertionStoreRequest>>
    {
        // Examine contracts that were called to see if they have assertions
        // associated, and dispatch accordingly
        let mut assertion_store_reader = self.assertion_store_reader.clone();

        let assertions = tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(async move {
                // TODO: improve channel error handling
                assertion_store_reader
                    .read(block_env.number, traces)
                    .await
                    .expect("Failed to send request")
                    .expect("Failed to receive response, channel empty and closed")
            })
        });

        Ok(assertions
            .into_par_iter()
            .map(move |assertion_contract| {
                self.run_assertion_contract(&assertion_contract, block_env.clone())
            })
            .flatten())
    }

    fn run_assertion_contract(
        &self,
        assertion_contract: &AssertionContract,
        block_env: BlockEnv,
    ) -> Vec<AssertionResult> {
        let AssertionContract {
            fn_selectors,
            code,
            code_hash,
        } = assertion_contract;

        trace!(?code_hash, "Running assertion contract");

        let mut db = self.db.clone();

        db.insert_account_info(
            Self::ASSERTION_CONTRACT_ADDRESS,
            AccountInfo::new(U256::ZERO, 0, *code_hash, code.clone()),
        );

        fn_selectors
            .into_par_iter()
            .map(|fn_selector: &FixedBytes<4>| {
                let mut evm = EvmBuilder::default()
                    .with_db(db.clone())
                    .with_block_env(block_env.clone())
                    .with_external_context(NoOpInspector)
                    .append_handler_register(inspector_handle_register)
                    .modify_tx_env(|env| {
                        *env = TxEnv {
                            transact_to: TxKind::Call(Self::ASSERTION_CONTRACT_ADDRESS),
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

    /// Commits a transaction against the underlying database, persisting the state.
    pub fn commit_tx(
        &mut self,
        block_env: BlockEnv,
        tx_env: TxEnv,
    ) -> Result<CallTracer, ExecutorError> {
        let mut evm = Evm::builder()
            .with_db(self.db.clone())
            .with_external_context(CallTracer::default())
            .with_block_env(block_env)
            .modify_tx_env(|env| *env = tx_env)
            .append_handler_register(inspector_handle_register)
            .build();

        let result = evm.transact()?;

        self.db.commit(result.state);

        Ok(evm.context.external)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_run_assertions_tx_state_persists() -> Result<(), Box<dyn std::error::Error>> {
    use super::test_utils::{
        counter_acct_info,
        counter_assertion,
        counter_call,
        COUNTER_ADDRESS,
    };
    use crate::{
        primitives::{
            BlockEnv,
            U256,
        },
        store::AssertionStore,
        AssertionExecutorBuilder,
    };

    use revm::{
        db::AccountState,
        primitives::uint,
        InMemoryDB,
    };

    let mut db = InMemoryDB::default();
    db.insert_account_info(COUNTER_ADDRESS, counter_acct_info());

    let assertion_store = AssertionStore::default();

    assertion_store
        .writer()
        .write(
            U256::ZERO,
            vec![(COUNTER_ADDRESS, vec![counter_assertion()])],
        )
        .await
        .unwrap();

    let mut executor = AssertionExecutorBuilder::new(db, assertion_store.reader()).build();

    let block_env = BlockEnv::default();
    let tx = counter_call();

    for (expected_state, expected_result) in [(uint!(1_U256), true), (uint!(2_U256), false)] {
        let traces = executor.commit_tx(block_env.clone(), tx.decoded.clone())?;

        let counter_account = executor.db.load_account(COUNTER_ADDRESS).unwrap();

        //Assert that the state of the counter contract is as expected
        assert_eq!(
            counter_account.storage.get(&U256::ZERO),
            Some(&expected_state)
        );

        //Assert that the assertions resolved as expected
        let result = executor
            .execute_assertions(block_env.clone(), traces.clone())
            .expect("receiver dropped");

        let success = result.all(|r| r.is_success());
        assert_eq!(success, expected_result);
    }
    //Assert that the assertion contract is not persisted in the database
    assert_eq!(
        executor
            .db
            .load_account(AssertionExecutor::<InMemoryDB>::ASSERTION_CONTRACT_ADDRESS)
            .unwrap()
            .account_state,
        AccountState::NotExisting
    );

    Ok(())
}
