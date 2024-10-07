pub mod fs;

mod memory_db;
pub use memory_db::MemoryDb;

mod shared_db;
pub use shared_db::SharedDB;

mod error;
pub use error::NotFoundError;

pub use revm::{
    db::CacheDB,
    DatabaseCommit,
    DatabaseRef,
};

pub trait PhDB: DatabaseRef + Clone + Sync + Send + std::fmt::Debug {}

impl<T> PhDB for T where T: DatabaseRef + Clone + Sync + Send + std::fmt::Debug {}
