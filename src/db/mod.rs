mod shared_db;
pub use shared_db::SharedDB;

mod memory_db;

mod error;
pub use error::NotFoundError;

pub use revm::{
    db::CacheDB,
    DatabaseCommit,
    DatabaseRef,
};

pub trait PhDB: DatabaseCommit + DatabaseRef + Clone + Sync + Send + std::fmt::Debug {}

impl<T> PhDB for T where T: DatabaseCommit + DatabaseRef + Clone + Sync + Send + std::fmt::Debug {}
