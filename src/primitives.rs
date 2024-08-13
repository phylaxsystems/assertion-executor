use crate::error::ExecutorError;
pub use revm::primitives::{
    address,
    AccountInfo,
    Address,
    BlockEnv,
    Bytecode,
    EVMError,
    ExecutionResult,
    FixedBytes,
    TxEnv,
    TxKind,
    B256,
    U256,
};

#[derive(Default, Debug, Clone, PartialEq)]
pub struct AssertionContract {
    pub fn_selectors: Vec<FixedBytes<4>>,
    pub code: Bytecode,
    pub code_hash: B256,
}

pub type BuiltBlock = (Vec<TransactionBundle>, BlockEnv);

/// Contains raw TxEnv as well as the received RPC call.
#[derive(Debug, Clone, Default)]
pub struct TransactionBundle {
    pub decoded: TxEnv,
    pub raw: String,
}

pub struct AssertionId {
    pub fn_selector: FixedBytes<4>,
    pub code_hash: B256,
}

pub struct AssertionResult {
    pub id: AssertionId,
    pub result: Result<ExecutionResult, ExecutorError>,
}

impl AssertionResult {
    pub fn is_success(&self) -> bool {
        self.result.is_ok() && self.result.as_ref().unwrap().is_success()
    }
}
