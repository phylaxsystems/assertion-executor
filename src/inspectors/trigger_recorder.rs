use crate::{
    inspectors::sol_primitives::{
        Error,
        Triggers,
    },
    primitives::{
        address,
        bytes,
        Address,
        Bytecode,
        Bytes,
        FixedBytes,
        U256,
    },
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
    Database,
    EvmContext,
    InMemoryDB,
    Inspector,
};

use alloy_sol_types::{
    SolCall,
    SolError,
};

use std::{
    collections::{
        HashMap,
        HashSet,
    },
    ops::Range,
};

/// Trigger recorder address
/// address(uint160(uint256(keccak256("TriggerRecorder"))))
pub const TRIGGER_RECORDER: Address = address!("55BB9AD8Dc1EE06D47279fC2B23Cd755B7f2d326");

/// Trigger type
#[derive(Clone, Debug, Default, PartialEq, Hash, Eq)]
pub enum TriggerType {
    #[default]
    Call,
}

/// TriggerRecorder is an inspector for recording calls made to register triggers at the trigger
/// recorder address.
/// The recorder triggers are used to determine when to run an assertion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriggerRecorder {
    pub triggers: HashMap<FixedBytes<4>, HashSet<TriggerType>>,
}

#[derive(thiserror::Error, Debug)]
pub enum RecordError {
    #[error("Failed to decode call inputs")]
    CallDecodeError(#[from] alloy_sol_types::Error),
    #[error("Fn selector not found")]
    FnSelectorNotFound,
}

impl TriggerRecorder {
    /// Records a trigger call made to the trigger recorder address.
    fn record_trigger(&mut self, inputs: &CallInputs) -> Result<(), RecordError> {
        match inputs
            .input
            .as_ref()
            .get(0..4)
            .unwrap_or_default()
            .try_into()
            .unwrap_or_default()
        {
            Triggers::registerCallTriggerCall::SELECTOR => {
                let fn_selector =
                    Triggers::registerCallTriggerCall::abi_decode(&inputs.input, true)?.fnSelector;

                self.triggers
                    .entry(fn_selector)
                    .or_default()
                    .insert(TriggerType::Call);
            }
            _ => return Err(RecordError::FnSelectorNotFound),
        };
        Ok(())
    }
}

/// Convert a fork result to a call outcome.
/// Uses the default require [`Error`] signature for encoding revert messages.
fn record_result_to_call_outcome(
    result: Result<(), RecordError>,
    gas: Gas,
    memory_offset: Range<usize>,
) -> CallOutcome {
    match result {
        Ok(()) => {
            CallOutcome {
                result: InterpreterResult {
                    result: InstructionResult::Stop,
                    output: Bytes::default(),
                    gas,
                },
                memory_offset,
            }
        }
        Err(e) => {
            CallOutcome {
                result: InterpreterResult {
                    result: InstructionResult::Revert,
                    output: Error::abi_encode(&Error { _0: e.to_string() }).into(),
                    gas,
                },
                memory_offset,
            }
        }
    }
}

impl<DB: Database> Inspector<DB> for TriggerRecorder {
    fn initialize_interp(&mut self, _interp: &mut Interpreter, _context: &mut EvmContext<DB>) {}

    fn step(&mut self, _interp: &mut Interpreter, _context: &mut EvmContext<DB>) {}

    fn step_end(&mut self, _interp: &mut Interpreter, _context: &mut EvmContext<DB>) {}

    fn call_end(
        &mut self,
        _context: &mut EvmContext<DB>,
        _inputs: &CallInputs,
        outcome: CallOutcome,
    ) -> CallOutcome {
        outcome
    }

    fn create_end(
        &mut self,
        _context: &mut EvmContext<DB>,
        _inputs: &CreateInputs,
        outcome: CreateOutcome,
    ) -> CreateOutcome {
        outcome
    }

    fn call(
        &mut self,
        _context: &mut EvmContext<DB>,
        inputs: &mut CallInputs,
    ) -> Option<CallOutcome> {
        if inputs.target_address == TRIGGER_RECORDER {
            let record_result = self.record_trigger(inputs);
            let gas = Gas::new(inputs.gas_limit);
            return Some(record_result_to_call_outcome(
                record_result,
                gas,
                inputs.return_memory_offset.clone(),
            ));
        }
        None
    }

    fn create(
        &mut self,
        _context: &mut EvmContext<DB>,
        _inputs: &mut CreateInputs,
    ) -> Option<CreateOutcome> {
        None
    }

    fn selfdestruct(&mut self, _contract: Address, _target: Address, _value: U256) {}
}

/// Insert the trigger recorder account into the database.
pub fn insert_trigger_recorder_account(db: &mut InMemoryDB) {
    db.insert_account_info(
        TRIGGER_RECORDER,
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
        build_evm::new_evm,
        primitives::{
            fixed_bytes,
            Bytecode,
            TxEnv,
            TxKind,
        },
        store::triggersCall,
        test_utils::deployed_bytecode,
    };
    use revm::InMemoryDB;

    #[cfg(feature = "optimism")]
    use crate::executor::config::create_optimism_fields;

    fn run_trigger_recorder_test(artifact: &str) -> TriggerRecorder {
        let assertion_contract = Address::random();
        let deployed_code = deployed_bytecode(&format!("{}.sol:{}", artifact, artifact));

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            assertion_contract,
            crate::primitives::AccountInfo {
                code: Some(Bytecode::new_raw(deployed_code)),
                ..Default::default()
            },
        );

        insert_trigger_recorder_account(&mut db);

        let tx_env = TxEnv {
            transact_to: TxKind::Call(assertion_contract),
            data: triggersCall::SELECTOR.into(),
            #[cfg(feature = "optimism")]
            optimism: create_optimism_fields(),
            ..Default::default()
        };

        let mut evm = new_evm(
            tx_env,
            Default::default(),
            Default::default(),
            Default::default(),
            &mut db,
            TriggerRecorder::default(),
        );

        let result = evm.transact().unwrap();

        assert!(
            result.result.is_success(),
            "Failed to transact: {:#?}",
            result
        );

        evm.context.external
    }

    #[test]
    fn trigger_recording() {
        let mut triggers: HashMap<FixedBytes<4>, HashSet<TriggerType>> = HashMap::new();
        triggers.insert(
            fixed_bytes!("DEADBEEF"),
            vec![TriggerType::Call].into_iter().collect(),
        );

        assert_eq!(
            run_trigger_recorder_test("CallTrigger"),
            TriggerRecorder { triggers }
        );
    }
}
