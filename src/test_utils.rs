#![cfg(any(test, feature = "test"))]

use crate::primitives::{
    AccountInfo,
    Address,
    AssertionContract,
    Bytecode,
    TxEnv,
    TxKind,
    U256,
};

use revm::primitives::{
    fixed_bytes,
    hex,
    keccak256,
    Bytes,
    FixedBytes,
    ResultAndState,
};

use crate::{
    db::SharedDB,
    primitives::{
        address,
        BlockEnv,
    },
    store::MockStore,
    AssertionExecutorBuilder,
};

/// Deployed bytecode of contract-mocks/src/SimpleCounterAssertion.sol:Counter
pub const COUNTER: &str = "SimpleCounterAssertion.sol:Counter";
pub const COUNTER_ADDRESS: Address = Address::new([1u8; 20]);

pub fn counter_call() -> TxEnv {
    TxEnv {
        transact_to: TxKind::Call(COUNTER_ADDRESS),
        data: fixed_bytes!("d09de08a").into(),
        ..TxEnv::default()
    }
}

pub fn counter_acct_info() -> AccountInfo {
    let code = deployed_bytecode(COUNTER);
    let code_hash = keccak256(&code);
    AccountInfo {
        balance: U256::ZERO,
        nonce: 1,
        code_hash,
        code: Some(Bytecode::LegacyRaw(code)),
    }
}

pub const SIMPLE_ASSERTION_COUNTER: &str = "SimpleCounterAssertion.sol:SimpleCounterAssertion";

pub fn counter_assertion() -> AssertionContract {
    let code = bytecode(SIMPLE_ASSERTION_COUNTER);
    let code_hash = keccak256(&code);
    AssertionContract {
        code: Bytecode::LegacyRaw(code),
        code_hash,
        fn_selectors: vec![fixed_bytes!("c667b77f")],
    }
}

pub const FN_SELECTOR: &str = "SelectorImpl.sol:SelectorImpl";
pub const BAD_FN_SELECTOR: &str = "SelectorImpl.sol:BadSelectorImpl";

pub fn selector_assertion() -> AssertionContract {
    let code = bytecode(FN_SELECTOR);
    let code_hash = keccak256(&code);

    AssertionContract {
        code: Bytecode::LegacyRaw(code),
        code_hash,
        fn_selectors: vec![
            fixed_bytes!("d210b7cf"),
            fixed_bytes!("e7f48038"),
            fixed_bytes!("1ff1bc3a"),
        ],
    }
}

/// Returns a random FixedBytes of length N
pub fn random_bytes<const N: usize>() -> FixedBytes<N> {
    let mut value = [0u8; N];
    value.iter_mut().for_each(|x| *x = rand::random());
    FixedBytes::new(value)
}

fn read_artifact(input: &str) -> serde_json::Value {
    let mut parts = input.split(':');
    let file_name = parts.next().expect("Failed to read filename");
    let contract_name = parts.next().expect("Failed to read contract name");
    let path = format!("contract-mocks/out/{}/{}.json", file_name, contract_name);

    let file = std::fs::File::open(path).expect("Failed to open file");
    serde_json::from_reader(file).expect("Failed to parse JSON")
}

/// Reads deployment bytecode from a contract-mocks artifact
///
/// # Arguments
/// * `input` - ${file_name}:${contract_name}
pub fn bytecode(input: &str) -> Bytes {
    let value = read_artifact(input);
    let bytecode = value["bytecode"]["object"]
        .as_str()
        .expect("Failed to read bytecode");
    hex::decode(bytecode)
        .expect("Failed to decode bytecode")
        .into()
}

/// Reads deployed bytecode from a contract-mocks artifact
///
/// # Arguments
/// * `input` - ${file_name}:${contract_name}
pub fn deployed_bytecode(input: &str) -> Bytes {
    let value = read_artifact(input);
    let bytecode = value["deployedBytecode"]["object"]
        .as_str()
        .expect("Failed to read bytecode");
    hex::decode(bytecode)
        .expect("Failed to decode bytecode")
        .into()
}

pub async fn run_precompile_test(artifact: &str) -> Option<ResultAndState> {
    let caller = address!("5fdcca53617f4d2b9134b29090c87d01058e27e9");
    let target = address!("118dd24a3b0d02f90d8896e242d3838b4d37c181");

    let db = SharedDB::<0>::new_test();

    let mut assertion_store = MockStore::default();

    let mut fork_db = db.fork();

    // Write test assertion to assertion store
    // bytecode of contract-mocks/src/GetLogsTest.sol:GetLogsTest
    let assertion_code = bytecode(&format!("{}.sol:{}", artifact, artifact));

    assertion_store
        .insert(target, vec![Bytecode::LegacyRaw(assertion_code)])
        .unwrap();

    let mut executor = AssertionExecutorBuilder::new(db, assertion_store.reader()).build();

    // Deploy mock using bytecode of contract-mocks/src/GetLogsTest.sol:Target
    let target_deployment_tx = TxEnv {
        caller,
        data: bytecode("Target.sol:Target"),
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
        data: bytecode(&format!("{}.sol:{}", artifact, "TriggeringTx")),
        transact_to: TxKind::Create,
        ..Default::default()
    };
    //Execute triggering tx.
    executor
        .validate_transaction(BlockEnv::default(), trigger_tx, &mut fork_db)
        .await
        .unwrap()
}
