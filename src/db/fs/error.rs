use std::io::Error as IoError;
/// Errors that can occur when interacting with the file-system database.
#[derive(thiserror::Error, Debug)]
pub enum FsDbError {
    /// An IO error occurred during a sled operation.
    #[error("Sled Io Error: {0}")]
    IoError(#[from] IoError),
}
