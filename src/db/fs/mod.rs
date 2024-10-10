pub mod fs_db;

pub(in crate::db) use fs_db::FsDb;

pub mod serde;

pub mod error;

pub use error::FsDbError;
