use crate::{db::Ext, store::handler::AssertionStoreRequest, AssertionExecutor};

use revm::db::{Database, DatabaseCommit};
use tokio::sync::mpsc::Sender;

pub struct AssertionExecutorBuilder<DB: Database + DatabaseCommit + Ext<DB>> {
    pub db: DB,
    pub assertion_store_tx: Sender<AssertionStoreRequest>,
}

//TODO: Extend with any necessary configuration
impl<DB: Database + DatabaseCommit + Ext<DB>> AssertionExecutorBuilder<DB> {
    pub fn new(db: DB, assertion_store_tx: Sender<AssertionStoreRequest>) -> Self {
        AssertionExecutorBuilder {
            db,
            assertion_store_tx,
        }
    }

    pub fn build(self) -> AssertionExecutor<DB> {
        let (assertion_match_tx, assertion_match_rx) = tokio::sync::mpsc::channel(1_000);
        AssertionExecutor {
            db: self.db,
            assertion_store_tx: self.assertion_store_tx,
            assertion_match_tx,
            assertion_match_rx,
        }
    }
}
