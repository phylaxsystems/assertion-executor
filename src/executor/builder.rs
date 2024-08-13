use crate::{
    db::Ext,
    store::AssertionStoreReader,
    AssertionExecutor,
};

use revm::db::{
    Database,
    DatabaseCommit,
};

pub struct AssertionExecutorBuilder<DB: Database + DatabaseCommit + Ext<DB>> {
    pub db: DB,
    pub assertion_store_reader: AssertionStoreReader,
}

//TODO: Extend with any necessary configuration
impl<DB: Database + DatabaseCommit + Ext<DB>> AssertionExecutorBuilder<DB> {
    pub fn new(db: DB, assertion_store_reader: AssertionStoreReader) -> Self {
        AssertionExecutorBuilder {
            db,
            assertion_store_reader,
        }
    }

    pub fn build(self) -> AssertionExecutor<DB> {
        AssertionExecutor {
            db: self.db,
            assertion_store_reader: self.assertion_store_reader,
        }
    }
}
