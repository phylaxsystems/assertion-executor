#![feature(test)]

mod error;
pub use error::ExecutorError;

mod executor;
pub use executor::{
    config::ExecutorConfig,
    AssertionExecutor,
};

pub mod primitives;

pub mod store;

pub mod inspectors;

pub mod db;

pub mod build_evm;

pub mod utils;

#[cfg(any(test, feature = "test"))]
pub mod test_utils;

#[cfg(feature = "phoundry")]
extern crate revm_18 as revm;

#[cfg(not(feature = "phoundry"))]
extern crate revm_17 as revm;
