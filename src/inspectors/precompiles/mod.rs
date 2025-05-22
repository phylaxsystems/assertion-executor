//! # `precompiles`
//!
//! The `precompiles` mod contains the implementations of all the phevm precompiles.
//! Helper methods used across precompiles can be found here, while the rest of
//! the precompile implementations can be found as follows:
//!
//! - `load`: Loads storage from any account.

pub mod assertion_adopter;
pub mod calls;
pub mod fork;
pub mod load;
pub mod logs;
pub mod state_changes;
