use crate::{
    primitives::EVMError,
    store::AssertionStoreReaderError,
};

use std::fmt::Debug;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum ExecutorError<DbError: Debug> {
    #[error("Expected value not found in database")]
    TxError(#[from] EVMError<DbError>),
    #[error("Failed to read assertions")]
    AssertionReadError(#[from] AssertionStoreReaderError),
}
