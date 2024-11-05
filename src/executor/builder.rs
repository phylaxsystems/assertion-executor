use crate::{
    db::PhDB,
    store::AssertionStoreReader,
    AssertionExecutor,
};
use revm::primitives::SpecId;

pub struct AssertionExecutorBuilder<DB> {
    pub db: DB,
    pub assertion_store_reader: AssertionStoreReader,
    pub spec_id: SpecId,
}

//TODO: Extend with any necessary configuration
impl<DB: PhDB> AssertionExecutorBuilder<DB> {
    pub fn new(db: DB, assertion_store_reader: AssertionStoreReader) -> Self {
        AssertionExecutorBuilder {
            db,
            assertion_store_reader,
            spec_id: SpecId::LATEST,
        }
    }

    /// Set the evm [`SpecId`] for the assertion executor
    pub fn with_spec_id(mut self, spec_id: SpecId) -> Self {
        self.spec_id = spec_id;
        self
    }

    /// Build the assertion executor
    pub fn build(self) -> AssertionExecutor<DB> {
        AssertionExecutor {
            db: self.db,
            assertion_store_reader: self.assertion_store_reader,
            spec_id: self.spec_id,
        }
    }
}
