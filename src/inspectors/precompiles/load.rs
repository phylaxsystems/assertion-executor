use crate::{
    db::MultiForkDb,
    inspectors::{
        phevm::PhEvm::loadCall,
        precompiles::empty_outcome,
    },
    primitives::Address,
    revm::DatabaseRef,
};

use revm::{
    interpreter::{
        CallInputs,
        CallOutcome,
        Gas,
        InstructionResult,
        InterpreterResult,
    },
    InnerEvmContext,
};

use alloy_sol_types::{
    SolCall,
    SolValue,
};

/// Returns a storage slot for a given address. Will return `0x0` if slot empty.
pub fn load_external_slot(
    context: &InnerEvmContext<&mut MultiForkDb<impl DatabaseRef>>,
    call_inputs: &CallInputs,
    gas: Gas,
) -> CallOutcome {
    let call = match loadCall::abi_decode(&call_inputs.input, true) {
        Ok(call) => call,
        Err(_) => return empty_outcome(gas),
    };
    let address: Address = call.target;

    let slot = call.slot;

    let slot_value = match context.db.active_db.storage_ref(address, slot.into()) {
        Ok(rax) => rax,
        Err(_) => return empty_outcome(gas),
    };

    let value = SolValue::abi_encode(&slot_value);

    CallOutcome {
        result: InterpreterResult {
            result: InstructionResult::Return,
            output: value.into(),
            gas,
        },
        memory_offset: call_inputs.return_memory_offset.clone(),
    }
}

#[cfg(test)]
mod test {
    use crate::{
        db::SharedDB,
        primitives::{
            address,
            BlockEnv,
            Bytecode,
            SpecId,
            TxEnv,
            TxKind,
            U256,
        },
        store::MockStore,
        test_utils::bytecode,
        AssertionExecutor,
    };

    #[tokio::test]
    async fn test_get_storage() {
        let caller = address!("5fdcca53617f4d2b9134b29090c87d01058e27e9");
        let target = address!("118dd24a3b0d02f90d8896e242d3838b4d37c181");

        let db = SharedDB::<0>::new_test();
        let mut assertion_store = MockStore::default();

        let assertion_code = bytecode("TestGetStorage.sol:GetStorageTest");
        let target_code = bytecode("TestGetStorage.sol:Target");

        assertion_store
            .insert(target, vec![Bytecode::LegacyRaw(assertion_code.clone())])
            .unwrap();

        let mut executor = AssertionExecutor {
            db: db.clone(),
            assertion_store_reader: assertion_store.reader(),
            spec_id: SpecId::LATEST,
            chain_id: 1,
        };

        let mut fork_db = db.fork();

        // Deploy mock using bytecode of contract-mocks/src/GetLogsTest.sol:Target
        let target_deployment_tx = TxEnv {
            caller,
            data: bytecode("TestGetStorage.sol:Target"),
            transact_to: TxKind::Create,
            ..Default::default()
        };

        // Execute target deployment tx
        executor
            .execute_forked_tx(BlockEnv::default(), target_deployment_tx, &mut fork_db)
            .unwrap();

        let trigger_tx = TxEnv {
            caller,
            data: bytecode("TestGetStorage.sol:TriggeringTx"),
            transact_to: TxKind::Create,
            ..Default::default()
        };

        let result = executor
            .validate_transaction(BlockEnv::default(), trigger_tx, &mut fork_db)
            .await
            .unwrap()
            .unwrap();

        println!("{:#?}", result);

        assert!(result.result.is_success());
    }
}
