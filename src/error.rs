use crate::{
    db::NotFoundError,
    primitives::EVMError,
    store::AssertionStoreRequest,
};

use std::fmt::Debug;
use thiserror::Error;
use tokio::sync::mpsc::error::SendError;

#[derive(Error, Debug)]
pub enum ExecutorError {
    #[error("Expected value not found in database")]
    TxError(#[from] EVMError<NotFoundError>),
    #[error("Assertion store send error")]
    AssertionStoreSendError(#[from] SendError<AssertionStoreRequest>),
}
