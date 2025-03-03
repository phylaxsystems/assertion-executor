use crate::{
    inspectors::sol_primitives::{
        Error,
        ITriggerRecorder,
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

use serde::{
    Deserialize,
    Serialize,
};

/// Trigger recorder address
/// address(uint160(uint256(keccak256("TriggerRecorder"))))
pub const TRIGGER_RECORDER: Address = address!("55BB9AD8Dc1EE06D47279fC2B23Cd755B7f2d326");

/// Trigger type represents different types of triggers that can be registered for assertions
///
/// Call { fn_selector: FixedBytes<4> } - Triggers on specific function calls matching the 4-byte selector
/// AllCalls - Triggers on any function call to the contract
/// BalanceChange - Triggers when the contract's ETH balance changes
/// StorageChange { slot: FixedBytes<32> } - Triggers when a specific storage slot is modified
/// AllStorageChanges - Triggers on any storage modification in the contract
///
/// These triggers are used to determine when an assertion should be executed.
/// The trigger recorder keeps track of which triggers are registered for each contract.
#[derive(Clone, Debug, PartialEq, Hash, Eq, Serialize, Deserialize)]
pub enum TriggerType {
    Call { fn_selector: FixedBytes<4> },
    AllCalls,
    BalanceChange,
    StorageChange { slot: FixedBytes<32> },
    AllStorageChanges,
}

/// TriggerRecorder is an inspector for recording calls made to register triggers at the trigger
/// recorder address.
/// The recorder triggers are used to determine when to run an assertion.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Clone)]
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
            ITriggerRecorder::registerCallTrigger_0Call::SELECTOR => {
                let call =
                    ITriggerRecorder::registerCallTrigger_0Call::abi_decode(&inputs.input, true)?;
                let triggers = self.triggers.entry(call.fnSelector).or_default();

                // Remove any specific Call triggers and replace with AllCalls
                triggers.retain(|t| !matches!(t, TriggerType::Call { .. }));
                triggers.insert(TriggerType::AllCalls);
            }

            ITriggerRecorder::registerCallTrigger_1Call::SELECTOR => {
                let call =
                    ITriggerRecorder::registerCallTrigger_1Call::abi_decode(&inputs.input, true)?;
                let triggers = self.triggers.entry(call.fnSelector).or_default();

                // If AllCalls exists, don't add specific call trigger
                if !triggers.contains(&TriggerType::AllCalls) {
                    triggers.insert(TriggerType::Call {
                        fn_selector: call.triggerSelector,
                    });
                }
            }

            ITriggerRecorder::registerStorageChangeTrigger_0Call::SELECTOR => {
                let call = ITriggerRecorder::registerStorageChangeTrigger_0Call::abi_decode(
                    &inputs.input,
                    true,
                )?;
                let triggers = self.triggers.entry(call.fnSelector).or_default();

                // Remove any specific StorageChange triggers and replace with AllStorageChanges
                triggers.retain(|t| !matches!(t, TriggerType::StorageChange { .. }));
                triggers.insert(TriggerType::AllStorageChanges);
            }

            ITriggerRecorder::registerStorageChangeTrigger_1Call::SELECTOR => {
                let call = ITriggerRecorder::registerStorageChangeTrigger_1Call::abi_decode(
                    &inputs.input,
                    true,
                )?;

                let triggers = self.triggers.entry(call.fnSelector).or_default();

                // If AllStorageChanges exists, don't add specific StorageChange trigger
                if !triggers.contains(&TriggerType::AllStorageChanges) {
                    triggers.insert(TriggerType::StorageChange { slot: call.slot });
                }
            }

            ITriggerRecorder::registerBalanceChangeTriggerCall::SELECTOR => {
                let fn_selector = ITriggerRecorder::registerBalanceChangeTriggerCall::abi_decode(
                    &inputs.input,
                    true,
                )?
                .fnSelector;

                self.triggers
                    .entry(fn_selector)
                    .or_default()
                    .insert(TriggerType::BalanceChange);
            }

            _ => return Err(RecordError::FnSelectorNotFound),
        }
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
    fn record_trigger_on_any() {
        let mut triggers: HashMap<FixedBytes<4>, HashSet<TriggerType>> = HashMap::new();
        triggers.insert(
            fixed_bytes!("DEADBEEF"),
            vec![
                TriggerType::AllCalls,
                TriggerType::AllStorageChanges,
                TriggerType::BalanceChange,
            ]
            .into_iter()
            .collect(),
        );

        assert_eq!(
            run_trigger_recorder_test("TriggerOnAny"),
            TriggerRecorder { triggers }
        );
    }

    #[test]
    fn record_trigger_on_specific() {
        let mut triggers: HashMap<FixedBytes<4>, HashSet<TriggerType>> = HashMap::new();
        triggers.insert(
            fixed_bytes!("DEADBEEF"),
            vec![
                TriggerType::Call {
                    fn_selector: fixed_bytes!("f18c388a"),
                },
                TriggerType::StorageChange {
                    slot: fixed_bytes!(
                        "ccc4fa32c72b32fc1388e9b17cbcd9cb5939d52551871739e4c3415f4ee595a0"
                    ),
                },
            ]
            .into_iter()
            .collect(),
        );

        assert_eq!(
            run_trigger_recorder_test("TriggerOnSpecific"),
            TriggerRecorder { triggers }
        );
    }

    #[test]
    fn record_trigger_override() {
        let mut triggers: HashMap<FixedBytes<4>, HashSet<TriggerType>> = HashMap::new();
        triggers.insert(
            fixed_bytes!("DEADBEEF"),
            vec![TriggerType::AllCalls, TriggerType::AllStorageChanges]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            run_trigger_recorder_test("TriggerOverride"),
            TriggerRecorder { triggers }
        );
    }
}
