use crate::{
    db::PhDB,
    store::AssertionStoreReader,
    AssertionExecutor,
};

pub struct AssertionExecutorBuilder<DB> {
    pub db: DB,
    pub assertion_store_reader: AssertionStoreReader,
}

//TODO: Extend with any necessary configuration
impl<DB: PhDB> AssertionExecutorBuilder<DB> {
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
