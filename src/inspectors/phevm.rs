use crate::{
    db::{
        multi_fork_db::MultiForkDb,
        DatabaseRef,
    },
    inspectors::{
        precompiles::calls::get_call_inputs,
        precompiles::fork::{
            fork_post_state,
            fork_pre_state,
        },
        precompiles::load::load_external_slot,
        precompiles::logs::get_logs,
        sol_primitives::PhEvm,
        tracer::CallTracer,
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
    Inspector,
};

use alloy_sol_types::SolCall;

/// Precompile address
/// address(uint160(uint256(keccak256("Kim Jong Un Sucks"))))
pub const PRECOMPILE_ADDRESS: Address = address!("4461812e00718ff8D80929E3bF595AEaaa7b881E");

#[derive(Debug)]
pub struct PhEvmContext<'a> {
    pub tx_logs: &'a [Log],
    pub call_traces: &'a CallTracer,
}

impl<'a> PhEvmContext<'a> {
    pub fn new(tx_logs: &'a [Log], call_traces: &'a CallTracer) -> Self {
        Self {
            tx_logs,
            call_traces,
        }
    }
}

/// PhEvmInspector is an inspector for supporting the PhEvm precompiles.
pub struct PhEvmInspector<'a> {
    init_journaled_state: JournaledState,
    context: &'a PhEvmContext<'a>,
}

impl<'a> PhEvmInspector<'a> {
    /// Create a new PhEvmInspector.
    pub fn new(
        spec_id: SpecId,
        db: &mut MultiForkDb<impl DatabaseRef>,
        context: &'a PhEvmContext<'a>,
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
            context,
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
                fork_pre_state(
                    &self.init_journaled_state,
                    context,
                    gas,
                    inputs.return_memory_offset.clone(),
                )
            }
            PhEvm::forkPostStateCall::SELECTOR => {
                fork_post_state(
                    &self.init_journaled_state,
                    context,
                    gas,
                    inputs.return_memory_offset.clone(),
                )
            }
            PhEvm::loadCall::SELECTOR => load_external_slot(&context.inner, inputs, gas),
            PhEvm::getLogsCall::SELECTOR => get_logs(inputs, self.context, gas),
            PhEvm::getCallInputsCall::SELECTOR => get_call_inputs(inputs, self.context, gas),
            _ => {
                CallOutcome {
                    result: InterpreterResult {
                        result: InstructionResult::Revert,
                        output: Bytes::default(),
                        gas,
                    },
                    memory_offset: inputs.return_memory_offset.clone(),
                }
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
