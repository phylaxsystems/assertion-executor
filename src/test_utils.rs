#![cfg(any(test, feature = "test"))]

use crate::primitives::{
    AccountInfo,
    Address,
    AssertionContract,
    Bytecode,
    TransactionBundle,
    TxEnv,
    TxKind,
    U256,
};
use revm::primitives::{
    bytes,
    fixed_bytes,
    keccak256,
    Bytes,
    FixedBytes,
};

pub const COUNTER_CODE : Bytes= bytes!("6080604052348015600f57600080fd5b506004361060325760003560e01c80638381f58a146037578063d09de08a146051575b600080fd5b603f60005481565b60405190815260200160405180910390f35b60576059565b005b600080549080606683606d565b9190505550565b600060018201608c57634e487b7160e01b600052601160045260246000fd5b506001019056fea2646970667358221220e286ad6519f82d6863d59b6024c676007347de449fb04de9aa0b47e14513e85a64736f6c63430008190033");
pub const COUNTER_ADDRESS: Address = Address::new([1u8; 20]);

pub fn counter_call() -> TransactionBundle {
    TransactionBundle {
        decoded: TxEnv {
            transact_to: TxKind::Call(COUNTER_ADDRESS),
            data: fixed_bytes!("d09de08a").into(),
            ..TxEnv::default()
        },
        raw: "".to_string(),
    }
}

pub fn counter_acct_info() -> AccountInfo {
    AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: keccak256(COUNTER_CODE),
        code: Some(Bytecode::LegacyRaw(COUNTER_CODE)),
    }
}

///Constructor code for the counter contract
pub const SIMPLE_ASSERTION_COUNTER_CODE: Bytes = bytes!("6080604052348015600f57600080fd5b506101948061001f6000396000f3fe608060405234801561001057600080fd5b506004361061002b5760003560e01c8063c667b77f14610030575b600080fd5b61003861003a565b005b60007301010101010101010101010101010101010101016001600160a01b0316638381f58a6040518163ffffffff1660e01b8152600401602060405180830381865afa15801561008e573d6000803e3d6000fd5b505050506040513d601f19601f820116820180604052508101906100b29190610145565b90507f4ae04e69a0231afea063135ce5674a4dae0944c876f7806737af2c1885a8a19a816040516100e591815260200190565b60405180910390a160018111156101425760405162461bcd60e51b815260206004820181905260248201527f436f756e7465722063616e6e6f742062652067726561746572207468616e2031604482015260640160405180910390fd5b50565b60006020828403121561015757600080fd5b505191905056fea26469706673582212203610a32c9af7e2bad371745ebe1938c9462244ca32f4fdae8c38b69f956281f164736f6c63430008190033");

pub fn counter_assertion() -> AssertionContract {
    AssertionContract {
        code: Bytecode::LegacyRaw(SIMPLE_ASSERTION_COUNTER_CODE),
        code_hash: keccak256(SIMPLE_ASSERTION_COUNTER_CODE),
        fn_selectors: vec![fixed_bytes!("c667b77f").into()],
    }
}

/// Returns a random FixedBytes of length N
pub fn random_bytes<const N: usize>() -> FixedBytes<N> {
    let mut value = [0u8; N];
    for i in 0..N {
        value[i] = rand::random();
    }
    FixedBytes::new(value)
}
