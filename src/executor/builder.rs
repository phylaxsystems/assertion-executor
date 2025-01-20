use crate::{
    db::PhDB,
    store::AssertionStoreReader,
    AssertionExecutor,
    DEFAULT_ASSERTION_GAS_LIMIT,
};
use revm::primitives::SpecId;

pub struct AssertionExecutorBuilder<DB> {
    pub db: DB,
    pub assertion_store_reader: AssertionStoreReader,
    pub spec_id: SpecId,
    pub chain_id: u64,
    pub assertion_gas_limit: u64,
}

//TODO: Extend with any necessary configuration
impl<DB: PhDB> AssertionExecutorBuilder<DB> {
    pub fn new(db: DB, assertion_store_reader: AssertionStoreReader) -> Self {
        AssertionExecutorBuilder {
            db,
            assertion_store_reader,
            spec_id: SpecId::LATEST,
            chain_id: 1,
            assertion_gas_limit: DEFAULT_ASSERTION_GAS_LIMIT,
        }
    }

    /// Set the assertion gas limit for the assertion executor
    pub fn with_assertion_gas_limit(mut self, gas_limit: u64) -> Self {
        self.assertion_gas_limit = gas_limit;
        self
    }

    /// Set the evm [`SpecId`] for the assertion executor
    pub fn with_spec_id(mut self, spec_id: SpecId) -> Self {
        self.spec_id = spec_id;
        self
    }

    /// Set the chain id for the assertion executor
    pub fn with_chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    /// Build the assertion executor
    pub fn build(self) -> AssertionExecutor<DB> {
        AssertionExecutor {
            db: self.db,
            assertion_store_reader: self.assertion_store_reader,
            spec_id: self.spec_id,
            chain_id: self.chain_id,
            assertion_gas_limit: self.assertion_gas_limit,
        }
    }
}
