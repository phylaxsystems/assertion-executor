use crate::{
    db::{
        multi_fork_db::{
            ForkError,
            ForkId,
            MultiForkDb,
        },
        DatabaseRef,
    },
    primitives::{
        address,
        bytes,
        Address,
        Bytecode,
        Bytes,
        JournaledState,
        U256,
    },
};

use alloy_sol_types::{
    sol,
    SolCall,
    SolError,
    SolValue,
};

use revm::{
    interpreter::{
        CallInputs,
        CallOutcome,
        CreateInputs,
        CreateOutcome,
        Gas,
        InstructionResult,
        Interpreter,
        InterpreterResult,
    },
    precompile::{
        PrecompileSpecId,
        Precompiles,
    },
    primitives::{
        Log,
        SpecId,
    },
    EvmContext,
    InnerEvmContext,
    Inspector,
};

/// Precompile address
/// address(uint160(uint256(keccak256("Kim Jong Un Sucks"))))
pub const PRECOMPILE_ADDRESS: Address = address!("4461812e00718ff8D80929E3bF595AEaaa7b881E");

sol! {
    interface PhEvm {

        // An Ethereum log
        struct Log {
            // The topics of the log, including the signature, if any.
            bytes32[] topics;
            // The raw data of the log.
            bytes data;
            // The address of the log's emitter.
            address emitter;
        }

        //Forks to the state prior to the assertion triggering transaction.
        function forkPreState() external;

        //Forks to the state after the assertion triggering transaction.
        function forkPostState() external;

        // Get the logs from the assertion triggering transaction.
        function getLogs() external returns (Log[] memory logs);
    }

    //Default require error
    error Error(string);
}

/// PhEvmInspector is an inspector for supporting the PhEvm precompiles.
pub struct PhEvmInspector<'a> {
    init_journaled_state: JournaledState,
    tx_logs: &'a [Log],
}

impl<'a> PhEvmInspector<'a> {
    /// Create a new PhEvmInspector.
    pub fn new(
        spec_id: SpecId,
        db: &mut MultiForkDb<impl DatabaseRef>,
        tx_logs: &'a [Log],
    ) -> Self {
        insert_precompile_account(db);

        let init_journaled_state = JournaledState::new(
            spec_id,
            Precompiles::new(PrecompileSpecId::from_spec_id(spec_id))
                .addresses()
                .copied()
                .collect(),
        );
        PhEvmInspector {
            init_journaled_state,
            tx_logs,
        }
    }

    /// Execute precompile functions for the PhEvm.
    pub fn execute_precompile(
        &self,
        context: &mut EvmContext<&mut MultiForkDb<impl DatabaseRef>>,
        inputs: &mut CallInputs,
    ) -> CallOutcome {
        let gas = Gas::new(inputs.gas_limit);

        match inputs
            .input
            .as_ref()
            .get(0..4)
            .unwrap_or_default()
            .try_into()
            .unwrap_or_default()
        {
            PhEvm::forkPreStateCall::SELECTOR => {
                let InnerEvmContext {
                    ref mut db,
                    ref mut journaled_state,
                    ..
                } = context.inner;

                let result =
                    db.switch_fork(ForkId::PreTx, journaled_state, &self.init_journaled_state);
                fork_result_to_call_outcome(result, gas)
            }
            PhEvm::forkPostStateCall::SELECTOR => {
                let InnerEvmContext {
                    ref mut db,
                    ref mut journaled_state,
                    ..
                } = context.inner;

                let result =
                    db.switch_fork(ForkId::PostTx, journaled_state, &self.init_journaled_state);

                fork_result_to_call_outcome(result, gas)
            }
            PhEvm::getLogsCall::SELECTOR => {
                let sol_logs: Vec<PhEvm::Log> = self
                    .tx_logs
                    .iter()
                    .map(|log| {
                        PhEvm::Log {
                            topics: log.topics().to_vec(),
                            data: log.data.data.clone(),
                            emitter: log.address,
                        }
                    })
                    .collect();

                CallOutcome {
                    result: InterpreterResult {
                        result: InstructionResult::Return,
                        output: SolValue::abi_encode(&sol_logs).into(),
                        gas,
                    },
                    memory_offset: inputs.return_memory_offset.clone(),
                }
            }
            _ => {
                CallOutcome {
                    result: InterpreterResult {
                        result: InstructionResult::Revert,
                        output: Bytes::default(),
                        gas,
                    },
                    memory_offset: 0..0,
                }
            }
        }
    }
}

/// Convert a fork result to a call outcome.
/// Uses the default require [`Error`] signature for encoding revert messages.
fn fork_result_to_call_outcome(result: Result<(), ForkError>, gas: Gas) -> CallOutcome {
    match result {
        Ok(()) => {
            CallOutcome {
                result: InterpreterResult {
                    result: InstructionResult::Stop,
                    output: Bytes::default(),
                    gas,
                },
                memory_offset: 0..0,
            }
        }
        Err(e) => {
            CallOutcome {
                result: InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Error::abi_encode(&Error { _0: e.to_string() }).into(),
                    gas,
                },
                memory_offset: 0..0,
            }
        }
    }
}

impl<DB: DatabaseRef> Inspector<&mut MultiForkDb<DB>> for PhEvmInspector<'_> {
    fn initialize_interp(
        &mut self,
        _interp: &mut Interpreter,
        _context: &mut EvmContext<&mut MultiForkDb<DB>>,
    ) {
    }

    fn step(&mut self, _interp: &mut Interpreter, _context: &mut EvmContext<&mut MultiForkDb<DB>>) {
    }

    fn step_end(
        &mut self,
        _interp: &mut Interpreter,
        _context: &mut EvmContext<&mut MultiForkDb<DB>>,
    ) {
    }

    fn call_end(
        &mut self,
        _context: &mut EvmContext<&mut MultiForkDb<DB>>,
        _inputs: &CallInputs,
        outcome: CallOutcome,
    ) -> CallOutcome {
        outcome
    }

    fn create_end(
        &mut self,
        _context: &mut EvmContext<&mut MultiForkDb<DB>>,
        _inputs: &CreateInputs,
        outcome: CreateOutcome,
    ) -> CreateOutcome {
        outcome
    }

    fn call(
        &mut self,
        context: &mut EvmContext<&mut MultiForkDb<DB>>,
        inputs: &mut CallInputs,
    ) -> Option<CallOutcome> {
        if inputs.target_address == PRECOMPILE_ADDRESS {
            return Some(self.execute_precompile(context, inputs));
        }
        None
    }

    fn create(
        &mut self,
        _context: &mut EvmContext<&mut MultiForkDb<DB>>,
        _inputs: &mut CreateInputs,
    ) -> Option<CreateOutcome> {
        None
    }

    fn selfdestruct(&mut self, _contract: Address, _target: Address, _value: U256) {}
}

/// Insert the precompile account into the database.
fn insert_precompile_account<T>(db: &mut MultiForkDb<T>) {
    db.active_db.insert_account_info(
        PRECOMPILE_ADDRESS,
        crate::primitives::AccountInfo {
            code: Some(Bytecode::new_raw(bytes!("45"))),
            ..Default::default()
        },
    );
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        db::SharedDB,
        primitives::{
            address,
            BlockEnv,
            Bytecode,
            TxEnv,
            TxKind,
        },
        store::MockStore,
        test_utils::bytecode,
        AssertionExecutor,
    };

    #[tokio::test]
    async fn test_fork_switching() {
        let caller = address!("5fdcca53617f4d2b9134b29090c87d01058e27e9");
        let target = address!("118dd24a3b0d02f90d8896e242d3838b4d37c181");

        let db = SharedDB::<0>::new_test();

        let mut assertion_store = MockStore::default();

        let mut fork_db = db.fork();

        // Write test assertion to assertion store
        // bytecode of contract-mocks/src/ForkTest.sol:ForkTest
        let assertion_code = bytecode("ForkTest.sol:ForkTest");

        assertion_store
            .insert(target, vec![Bytecode::LegacyRaw(assertion_code)])
            .unwrap();

        let mut executor = AssertionExecutor {
            db,
            assertion_store_reader: assertion_store.reader(),
            spec_id: SpecId::LATEST,
            chain_id: 1,
        };

        // Deploy mock using bytecode of contract-mocks/src/ForkTest.sol:Target
        let target_deployment_tx = TxEnv {
            caller,
            data: bytecode("ForkTest.sol:Target"),
            transact_to: TxKind::Create,
            ..Default::default()
        };

        // Execute target deployment tx
        executor
            .execute_forked_tx(BlockEnv::default(), target_deployment_tx, &mut fork_db)
            .unwrap();

        // Deploy TriggeringTx contract using bytecode of
        // contract-mocks/src/ForkTest.sol:TriggeringTx
        let trigger_tx = TxEnv {
            caller,
            data: bytecode("ForkTest.sol:TriggeringTx"),
            transact_to: TxKind::Create,
            ..Default::default()
        };

        //Execute triggering tx.
        assert!(executor
            .validate_transaction(BlockEnv::default(), trigger_tx, &mut fork_db)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn test_get_logs() {
        let caller = address!("5fdcca53617f4d2b9134b29090c87d01058e27e9");
        let target = address!("118dd24a3b0d02f90d8896e242d3838b4d37c181");

        let db = SharedDB::<0>::new_test();

        let mut assertion_store = MockStore::default();

        let mut fork_db = db.fork();

        // Write test assertion to assertion store
        // bytecode of contract-mocks/src/GetLogsTest.sol:GetLogsTest
        let assertion_code = bytecode("GetLogsTest.sol:GetLogsTest");

        assertion_store
            .insert(target, vec![Bytecode::LegacyRaw(assertion_code)])
            .unwrap();

        let mut executor = AssertionExecutor {
            db,
            assertion_store_reader: assertion_store.reader(),
            spec_id: SpecId::LATEST,
            chain_id: 1,
        };

        // Deploy mock using bytecode of contract-mocks/src/GetLogsTest.sol:Target
        let target_deployment_tx = TxEnv {
            caller,
            data: bytecode("GetLogsTest.sol:Target"),
            transact_to: TxKind::Create,
            ..Default::default()
        };

        // Execute target deployment tx
        executor
            .execute_forked_tx(BlockEnv::default(), target_deployment_tx, &mut fork_db)
            .unwrap();

        // Deploy TriggeringTx contract using bytecode of
        // contract-mocks/src/GetLogsTest.sol:TriggeringTx
        let trigger_tx = TxEnv {
            caller,
            data: bytecode("GetLogsTest.sol:TriggeringTx"),
            transact_to: TxKind::Create,
            ..Default::default()
        };
        //Execute triggering tx.
        assert!(executor
            .validate_transaction(BlockEnv::default(), trigger_tx, &mut fork_db)
            .await
            .unwrap()
            .is_some());
    }
}
