use crate::primitives::Address;
use alloy_provider::Provider;
use alloy_sol_types::sol;
use std::thread::JoinHandle;

sol! {
    // Events to index
    event AssertionAdded(address contractAddress, bytes32 assertionId, uint256 activeAtBlock);

    event AssertionRemoved(address contractAddress, bytes32 assertionId, uint256 activeAtBlock);

}

/// An assertion modification event
pub enum AssertionModificationEvent {
    Add(AssertionAdded),
    Remove(AssertionRemoved),
}

/// An assertion store that is populated via event indexing over rpc
#[allow(dead_code)]
pub struct RpcStore<P: Provider> {
    provider: P,
    state_oracle: Address,
    db: sled::Db,
}

#[allow(dead_code)]
impl<P: Provider> RpcStore<P> {
    /// Creates a new `RpcStore` with the given provider and state oracle address
    pub fn initialize(_provider: P, _state_oracle: Address) -> JoinHandle<()> {
        #[allow(clippy::empty_loop)]
        std::thread::spawn(move || loop {})
    }
}
