//! # `demo_lending`
//!
//! The following contains a sample lending protocol vulnerable to reentrancy.
//! The protocol accepts eth deposits, withdrawal and borrowing.
//!
//! The assertion checks that our output balance is not more than our balance
//! deposited inside of the protocol.

use assertion_executor::{
    db::SharedDB,
    primitives::{
        uint,
        Account,
        AccountInfo,
        AccountStatus,
        Address,
        BlockChanges,
        Bytecode,
        TxEnv,
        TxKind,
        U256,
    },
    test_utils::*,
    AssertionExecutor,
};

use revm::primitives::{
    address,
    bytes,
    keccak256,
};

/// Address of the lending contract
pub const LENDING_ADDRESS: Address = address!("118DD24a3b0D02F90D8896E242D3838B4D37c181");

/// Deployed bytecode of contract-mocks/src/DemoLending.sol:DemoLending
pub const LENDING: &str = "DemoLending.sol:DemoLending";

pub fn lending_acct_info() -> AccountInfo {
    let code = deployed_bytecode(LENDING);
    let code_hash = keccak256(&code);
    AccountInfo {
        balance: U256::MAX,
        nonce: 0,
        code_hash,
        code: Some(Bytecode::LegacyRaw(code)),
    }
}

pub fn lending_call_invalidative() -> TxEnv {
    TxEnv {
        transact_to: TxKind::Create,
        data: bytes!("60806040523473118dd24a3b0d02f90d8896e242d3838b4d37c181632e1a7d4d602f83670de0b6b3a76400006081565b6040518263ffffffff1660e01b8152600401604c91815260200190565b600060405180830381600087803b158015606557600080fd5b505af11580156078573d6000803e3d6000fd5b505050505060a7565b8082018082111560a157634e487b7160e01b600052601160045260246000fd5b92915050565b603f8060b46000396000f3fe6080604052600080fdfea2646970667358221220e43fbc2a652f02721df8672422ae04e2ea5bf5ec3b38139ec14145ffb273b64b64736f6c63430008190033"),
        value: uint!(10000000000000000000_U256),
        ..TxEnv::default()
    }
}

pub fn lending_call_valid() -> TxEnv {
    TxEnv {
        transact_to: TxKind::Create,
        data: bytes!("6080604052603f8060116000396000f3fe6080604052600080fdfea26469706673582212204218d98e4c98c70f0ec816e3e3a3accbe06687c57d73ddc5f00c71ad5d973f6f64736f6c63430008190033"),
        value: uint!(10000000000000000000_U256),
        ..TxEnv::default()
    }
}

/// Deploys the fork test contract.
pub fn deploy_demo_lending(
    _executor: &mut AssertionExecutor<SharedDB<5>>,
    block_changes: &mut BlockChanges,
) {
    // Fork test contract to block_changes
    block_changes.state_changes.insert(
        LENDING_ADDRESS,
        Account {
            info: lending_acct_info(),
            status: AccountStatus::Touched,
            ..Default::default()
        },
    );
}
