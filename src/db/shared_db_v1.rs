use crate::db::{
    NotFoundError,
    DB,
};
use crate::primitives::{
    AccountInfo,
    Address,
    Bytecode,
    B256,
    U256,
};
use revm::DatabaseRef;
use std::sync::{
    Arc,
    RwLock,
};

/// implement DatabaseRef trait from revm. Only return the latest 256 blockhashes.
pub struct SharedDB {
    db: Arc<RwLock<DB>>,
}

