use crate::primitives::EVMError;
use std::{convert::Infallible, fmt::Debug};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExecutorError {
    #[error("EVM error")]
    TxError(#[from] EVMError<Infallible>),
}
