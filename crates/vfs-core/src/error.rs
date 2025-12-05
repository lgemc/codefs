//! Error types for the virtual filesystem.

use thiserror::Error;

/// Errors that can occur in the virtual filesystem.
#[derive(Error, Debug, Clone)]
pub enum VfsError {
    /// The requested path was not found.
    #[error("Path not found: {0}")]
    NotFound(String),

    /// The path already exists.
    #[error("Path already exists: {0}")]
    AlreadyExists(String),

    /// The path is not a directory.
    #[error("Not a directory: {0}")]
    NotADirectory(String),

    /// The path is not a file.
    #[error("Not a file: {0}")]
    NotAFile(String),

    /// Invalid path.
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    /// Parent directory does not exist.
    #[error("Parent directory does not exist: {0}")]
    ParentNotFound(String),
}
