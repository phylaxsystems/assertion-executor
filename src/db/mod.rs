pub mod fs;

pub mod fork_db;

pub mod multi_fork_db;

mod memory_db;
pub use memory_db::MemoryDb;

mod shared_db;
pub use shared_db::SharedDB;

mod error;
pub use error::NotFoundError;

pub use revm::{
    Database,
    DatabaseCommit,
    DatabaseRef,
};

pub trait PhDB: DatabaseRef + Sync + Send {}

impl<T> PhDB for T where T: DatabaseRef + Sync + Send {}
