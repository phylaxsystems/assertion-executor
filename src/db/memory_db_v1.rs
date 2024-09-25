use crate::primitives::{
    AccountInfo,
    Address,
    BlockChanges,
    Bytecode,
    ValueHistory,
    B256,
    U256,
};
use std::collections::{
    HashMap,
    VecDeque,
};

pub struct DB {
    pub(super) storage: HashMap<Address, HashMap<U256, ValueHistory<U256>>>,
    pub(super) basic: HashMap<Address, ValueHistory<AccountInfo>>,
    pub(super) block_hashes: HashMap<u64, B256>,
    pub(super) code_by_hash: HashMap<B256, ValueHistory<Bytecode>>,

    pub(super) canonical_block_hash: B256,
    pub(super) canonical_block_num: u64,

    /// Block Changes for the blocks that have not been pruned.
    pub(super) block_changes: VecDeque<BlockChanges>,
}
