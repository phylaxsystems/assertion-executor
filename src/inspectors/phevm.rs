use crate::primitives::{
    address,
    Address,
    Bytes,
    U256,
};

use alloy_sol_types::{
    sol,
    SolCall,
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
    Inspector,
};

sol! {
    interface PhEvm {
        //Forks to the state prior to the assertion triggering transaction.
        function forkPreState() external;

        //Forks to the state after the assertion triggering transaction.
        function forkPostState() external;
    }
}

#[derive(Clone, Debug, Default)]
pub struct PhEvmInspector {}

impl PhEvmInspector {
    /// Execute precompile functions for the PhEvm.
    pub fn execute_precompile(
        &self,
        context: &mut EvmContext<impl Database>,
        inputs: &mut CallInputs,
    ) -> CallOutcome {
        match inputs
            .input
            .as_ref()
            .get(0..4)
            .unwrap_or_default()
            .try_into()
            .unwrap_or_default()
        {
            PhEvm::forkPreStateCall::SELECTOR => {
                unimplemented!();
            }
            PhEvm::forkPostStateCall::SELECTOR => {
                unimplemented!();
            }
            _ => {
                CallOutcome {
                    result: InterpreterResult {
                        result: InstructionResult::Revert,
                        output: Bytes::default(),
                        gas: Gas::default(),
                    },
                    memory_offset: 0..0,
                }
            }
        }
    }

    /// Precompile address
    /// address(uint160(uint256(keccak256("Kim Jong Un Sucks"))))
    const PRECOMPILE_ADDRESS: Address = address!("4461812e00718ff8D80929E3bF595AEaaa7b881E");
}

impl<DB: Database> Inspector<DB> for PhEvmInspector {
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
        context: &mut EvmContext<DB>,
        inputs: &mut CallInputs,
    ) -> Option<CallOutcome> {
        if inputs.target_address == Self::PRECOMPILE_ADDRESS {
            return Some(self.execute_precompile(context, inputs));
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
