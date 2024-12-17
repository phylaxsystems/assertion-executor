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
