use crate::{
    inspectors::TriggerType,
    primitives::{
        Address,
        FixedBytes,
        JournalEntry,
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
        Interpreter,
    },
    Database,
    EvmContext,
    Inspector,
};
use std::collections::{
    HashMap,
    HashSet,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallTracer {
    pub call_inputs: HashMap<(Address, FixedBytes<4>), Vec<CallInputs>>,
    pub journaled_state: Option<JournaledState>,
}

impl CallTracer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_call(&mut self, mut inputs: CallInputs) {
        // If the input is at least 4 bytes long, use the first 4 bytes as the selector
        // Otherwise, use 0x00000000 as the default selector
        // Note: It doesn't mean that the selector is a valid function selector of the target contract
        // but the goal is to have actual function selectors available for filtering in the getCall precompile
        let selector = if inputs.input.len() >= 4 {
            FixedBytes::from_slice(&inputs.input[..4])
        } else {
            FixedBytes::default() // 0x00000000 for ETH transfers/no-input calls
        };

        if inputs.input.len() >= 4 {
            inputs.input = revm::primitives::Bytes::from(inputs.input[4..].to_vec());
        }

        self.call_inputs
            .entry((inputs.target_address, selector))
            .or_default()
            .push(inputs);
    }

    pub fn calls(&self) -> HashSet<Address> {
        // TODO: Think about storing the call targets in a set in addition to the call inputs
        // to see if it improves performance
        self.call_inputs.keys().map(|(addr, _)| *addr).collect()
    }

    #[cfg(any(test, feature = "test"))]
    pub fn insert_trace(&mut self, address: Address) {
        self.call_inputs
            .insert((address, FixedBytes::default()), vec![]);
    }

    pub fn triggers(&self) -> HashMap<Address, HashSet<TriggerType>> {
        let mut result: HashMap<Address, HashSet<TriggerType>> = HashMap::new();

        // Record call triggers
        for (addr, selector) in self.call_inputs.keys() {
            result.entry(*addr).or_default().insert(TriggerType::Call {
                trigger_selector: *selector,
            });
        }

        // Process journal entries for balance changes
        if let Some(journaled_state) = &self.journaled_state {
            // Flatten the two-dimensional journal array
            for entry in journaled_state.journal.iter().flatten() {
                match entry {
                    JournalEntry::BalanceTransfer {
                        from,
                        to,
                        balance: _,
                    } => {
                        // Add balance change trigger for both from and to addresses
                        for addr in [from, to] {
                            result
                                .entry(*addr)
                                .or_default()
                                .insert(TriggerType::BalanceChange);
                        }
                    }

                    JournalEntry::StorageChanged {
                        address,
                        key,
                        had_value: _,
                    } => {
                        result
                            .entry(*address)
                            .or_default()
                            .insert(TriggerType::StorageChange {
                                trigger_slot: (*key).into(),
                            });
                    }
                    _ => {} // Ignore other journal entry types
                }
            }
        }
        result
    }
}

impl<DB: Database> Inspector<DB> for CallTracer {
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
        self.record_call(inputs.clone());
        None
    }

    fn create(
        &mut self,
        _context: &mut EvmContext<DB>,
        _inputs: &mut CreateInputs,
    ) -> Option<CreateOutcome> {
        //Maybe we cache contracts created by a transaction with assertions?
        None
    }

    fn selfdestruct(&mut self, _contract: Address, _target: Address, _value: U256) {}
}

#[cfg(test)]
mod test {
    use super::*;
    #[cfg(feature = "optimism")]
    use crate::executor::config::create_optimism_fields;
    use crate::{
        build_evm::new_tx_fork_evm,
        primitives::{
            address,
            Bytecode,
        },
        test_utils::deployed_bytecode,
    };
    use revm::{
        inspector_handle_register,
        primitives::{
            bytes,
            BlockEnv,
            SpecId,
            TxEnv,
        },
        Evm,
        InMemoryDB,
    };
    #[test]
    fn call_tracing() {
        let callee = address!("5fdcca53617f4d2b9134b29090c87d01058e27e9");

        // https://github.com/bluealloy/revm/issues/277
        // checks this use case
        let mut evm = Evm::builder()
            .with_db(InMemoryDB::default())
            .modify_db(|db| {
                let code = bytes!("5b597fb075978b6c412c64d169d56d839a8fe01b3f4607ed603b2c78917ce8be1430fe6101e8527ffe64706ecad72a2f5c97a95e006e279dc57081902029ce96af7edae5de116fec610208527f9fc1ef09d4dd80683858ae3ea18869fe789ddc365d8d9d800e26c9872bac5e5b6102285260276102485360d461024953601661024a53600e61024b53607d61024c53600961024d53600b61024e5360b761024f5360596102505360796102515360a061025253607261025353603a6102545360fb61025553601261025653602861025753600761025853606f61025953601761025a53606161025b53606061025c5360a661025d53602b61025e53608961025f53607a61026053606461026153608c6102625360806102635360d56102645360826102655360ae61026653607f6101e8610146610220677a814b184591c555735fdcca53617f4d2b9134b29090c87d01058e27e962047654f259595947443b1b816b65cdb6277f4b59c10a36f4e7b8658f5a5e6f5561");
                let info = crate::primitives::AccountInfo {
                    balance: "0x100c5d668240db8e00".parse().unwrap(),
                    code_hash: revm::primitives::keccak256(&code),
                    code: Some(Bytecode::new_raw(code.clone())),
                    nonce: 1,
                };
                db.insert_account_info(callee, info);
            })
            .modify_tx_env(|tx| {
                tx.caller = address!("5fdcca53617f4d2b9134b29090c87d01058e27e0");
                tx.transact_to = crate::primitives::TxKind::Call(callee);
                tx.data = revm::primitives::Bytes::new();
                tx.value = crate::primitives::U256::ZERO;
                #[cfg(feature = "optimism")] {
                    tx.optimism = create_optimism_fields();
                }
            })
            .with_external_context(CallTracer::default())
            .with_spec_id(SpecId::BERLIN)
            .append_handler_register(inspector_handle_register)
            .build();

        evm.transact().expect("Transaction to work");

        let expected = HashSet::from_iter(vec![callee; 33]);
        assert_eq!(evm.context.external.calls(), expected);
    }

    #[test]
    fn extract_triggers() {
        let callee = address!("5fdcca53617f4d2b9134b29090c87d01058e27e9");
        let code = deployed_bytecode(&format!("{}.sol:{}", "TriggerContract", "TriggerContract"));

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            callee,
            crate::primitives::AccountInfo {
                balance: "0x100c5d668240db8e00".parse().unwrap(),
                code_hash: revm::primitives::keccak256(&code),
                code: Some(Bytecode::new_raw(code.clone())),
                nonce: 1,
            },
        );

        let fn_selector: FixedBytes<4> =
            FixedBytes::<4>::from_slice(&revm::primitives::keccak256("trigger()")[..4]);

        let tx_env = TxEnv {
            caller: address!("5fdcca53617f4d2b9134b29090c87d01058e27e0"),
            transact_to: crate::primitives::TxKind::Call(callee),
            data: revm::primitives::Bytes::from(fn_selector.to_vec()),
            value: crate::primitives::U256::ZERO,
            #[cfg(feature = "optimism")]
            optimism: create_optimism_fields(),
            ..Default::default()
        };

        let mut evm = new_tx_fork_evm(
            tx_env,
            BlockEnv::default(),
            Default::default(),
            Default::default(),
            &mut db,
            CallTracer::default(),
        );

        evm.transact().expect("Transaction to work");

        let expected_triggers_trigger_contract: HashSet<TriggerType> = HashSet::from_iter(vec![
            TriggerType::Call {
                trigger_selector: fn_selector,
            },
            TriggerType::StorageChange {
                trigger_slot: U256::from(0).into(),
            },
            TriggerType::StorageChange {
                trigger_slot: U256::from(1).into(),
            },
            TriggerType::BalanceChange,
        ]);
        assert_eq!(
            *evm.context.external.triggers().entry(callee).or_default(),
            expected_triggers_trigger_contract
        );
    }
}
