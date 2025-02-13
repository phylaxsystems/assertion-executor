use revm::{
    inspector_handle_register,
    interpreter::{
        gas,
        instructions::host::{
            sload,
            sstore,
        },
        opcode::{
            SLOAD,
            SSTORE,
        },
        Gas,
        Host,
        Interpreter,
    },
    primitives::{
        spec_to_generic,
        BlockEnv,
        CfgEnv,
        Env,
        EnvWithHandlerCfg,
        Spec,
        SpecId,
        TxEnv,
    },
    Context,
    Database,
    Evm,
    EvmContext,
    Handler,
    Inspector,
};

pub fn new_evm<'evm, 'db, DB: Database, I: Inspector<&'db mut DB>>(
    tx_env: TxEnv,
    block_env: BlockEnv,
    chain_id: u64,
    spec_id: SpecId,
    db: &'db mut DB,
    inspector: I,
    reprice_storage: bool,
) -> Evm<'evm, I, &'db mut DB> {
    let mut cfg_env = CfgEnv::default();
    cfg_env.chain_id = chain_id;

    let env = Env {
        block: block_env,
        tx: tx_env,
        cfg: cfg_env,
    };

    let EnvWithHandlerCfg { env, handler_cfg } =
        EnvWithHandlerCfg::new_with_spec_id(Box::new(env), spec_id);

    let mut handler = Handler::new(handler_cfg);

    if reprice_storage {
        handler
            .instruction_table
            .insert(SLOAD, spec_to_generic!(spec_id, ph_sload::<_, SPEC>));

        handler
            .instruction_table
            .insert(SSTORE, spec_to_generic!(spec_id, ph_sstore::<_, SPEC>));
    }

    handler.append_handler_register_plain(inspector_handle_register);

    let context = Context::new(EvmContext::new_with_env(db, env), inspector);

    Evm::new(context, handler)
}

/// Reprice the gas of an operation to a fixed cost.
macro_rules! reprice_gas {
    ($interpreter:expr,$host:expr, $operation:expr, $gas:expr) => {{
        // Spend the new expected gas. Will revert execution with an out-of-gas error if the gas
        // limit is exceeded.
        gas!($interpreter, $gas);

        // Cache the expected gas outcome, and replace the gas with the maximum value so that gas
        // limits are not enforced against other costs.
        let gas = std::mem::replace(&mut $interpreter.gas, Gas::new(u64::MAX));
        $operation($interpreter, $host);

        $interpreter.gas = gas;
    }};
}

/// Reprice the SLOAD operation to 100 gas.
pub fn ph_sload<H, S: Spec>(interpreter: &mut Interpreter, host: &mut H)
where
    H: Host + ?Sized,
{
    reprice_gas!(interpreter, host, sload::<H, S>, 100);
}

/// Reprice the SSTORE operation to 100 gas.
pub fn ph_sstore<H, S: Spec>(interpreter: &mut Interpreter, host: &mut H)
where
    H: Host + ?Sized,
{
    reprice_gas!(interpreter, host, sstore::<H, S>, 100);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        primitives::{
            keccak256,
            AccountInfo,
            Address,
            BlockEnv,
            Bytecode,
            Bytes,
            EvmExecutionResult,
            TxEnv,
            TxKind,
            U256,
        },
        test_utils::deployed_bytecode,
    };
    use revm::db::InMemoryDB;
    use revm::inspectors::NoOpInspector;

    #[cfg(feature = "optimism")]
    use crate::executor::config::create_optimism_fields;

    fn setup_evm<'evm, 'db>(
        db: &'db mut InMemoryDB,
        tx_env: TxEnv,
        reprice_storage: bool,
    ) -> Evm<'evm, NoOpInspector, &'db mut InMemoryDB> {
        let inspector = crate::revm::inspectors::NoOpInspector;
        let block_env = BlockEnv {
            basefee: U256::from(1),
            ..Default::default()
        };
        let chain_id = 1;
        let spec_id = SpecId::default();
        new_evm(
            tx_env,
            block_env,
            chain_id,
            spec_id,
            db,
            inspector,
            reprice_storage,
        )
    }

    fn insert_caller(db: &mut InMemoryDB, caller: Address) {
        db.insert_account_info(
            caller,
            AccountInfo {
                nonce: 0,
                balance: U256::MAX,
                code_hash: keccak256(&[]),
                code: None,
            },
        );
    }

    fn insert_test_contract(db: &mut InMemoryDB, address: Address, code: Bytes) {
        db.insert_account_info(
            address,
            AccountInfo {
                nonce: 1,
                balance: U256::ZERO,
                code_hash: keccak256(&code),
                code: Some(Bytecode::LegacyRaw(code)),
            },
        );
    }

    fn run_test(contract: &str, with_reprice: bool, gas_limit: Option<u64>) -> EvmExecutionResult {
        let address = Address::random();
        let caller = Address::random();

        let mut db = InMemoryDB::default();
        insert_caller(&mut db, caller);

        insert_test_contract(&mut db, address, deployed_bytecode(contract));

        let tx_env = TxEnv {
            transact_to: TxKind::Call(address),
            caller: caller.clone(),
            gas_price: U256::from(1),
            gas_limit: gas_limit.unwrap_or(1_000_000),
            #[cfg(feature = "optimism")]
            optimism: create_optimism_fields(),
            ..Default::default()
        };

        let mut evm = setup_evm(&mut db, tx_env.clone(), with_reprice);
        evm.transact().unwrap().result
    }

    fn test_diff(contract: &str, expected_gas: u64) {
        let with_reprice_result = run_test(contract, true, None);
        let without_reprice_result = run_test(contract, false, None);
        assert_eq!(
            without_reprice_result.gas_used() - with_reprice_result.gas_used(),
            expected_gas
        );
    }

    fn test_at_limit(contract: &str) {
        let no_reprice_gas = run_test(contract, false, None).gas_used();
        let with_reprice_result = run_test(contract, false, Some(no_reprice_gas));
        assert!(with_reprice_result.is_success());
    }
    fn test_under_limit(contract: &str) {
        let no_reprice_gas = run_test(contract, false, None).gas_used();
        let with_reprice_result = run_test(contract, false, Some(no_reprice_gas - 1));
        assert!(with_reprice_result.is_halt());
        assert_eq!(with_reprice_result.gas_used(), no_reprice_gas - 1);
    }

    #[test]
    fn test_sload() {
        test_diff("StorageGas.sol:SLOADGas", 2_000);
    }

    #[test]
    fn test_sload_at_limit() {
        test_at_limit("StorageGas.sol:SLOADGas");
    }

    #[test]
    fn test_sload_under_limit() {
        test_under_limit("StorageGas.sol:SLOADGas");
    }

    #[test]
    fn test_sstore() {
        test_diff("StorageGas.sol:SSTOREGas", 22_000);
    }

    #[test]
    fn test_sstore_at_limit() {
        test_at_limit("StorageGas.sol:SSTOREGas");
    }

    #[test]
    fn test_sstore_under_limit() {
        test_under_limit("StorageGas.sol:SSTOREGas");
    }
}
