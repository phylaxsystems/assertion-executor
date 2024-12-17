use crate::{
    db::NotFoundError,
    primitives::EVMError,
    store::AssertionStoreReaderError,
};

use std::fmt::Debug;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExecutorError {
    #[error("Expected value not found in database")]
    TxError(#[from] EVMError<NotFoundError>),
    #[error("Failed to read assertions")]
    AssertionReadError(#[from] AssertionStoreReaderError),
}
