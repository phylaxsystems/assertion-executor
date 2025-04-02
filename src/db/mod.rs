pub mod fs;

pub mod fork_db;

pub mod multi_fork_db;
pub use multi_fork_db::MultiForkDb;

mod memory_db;
pub use memory_db::MemoryDb;

mod shared_db;
pub use shared_db::SharedDB;

pub mod overlay;

mod error;
pub use error::NotFoundError;

pub use revm::db::{
    Database,
    DatabaseCommit,
    DatabaseRef,
};

pub trait PhDB: DatabaseRef + Sync + Send {}

impl<T> PhDB for T where T: DatabaseRef + Sync + Send {}
