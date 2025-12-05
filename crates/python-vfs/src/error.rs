//! Error types for the Python VFS.

use thiserror::Error;

/// Errors that can occur in the Python VFS.
#[derive(Error, Debug)]
pub enum PythonVfsError {
    /// Failed to parse Python code.
    #[error("Parse error: {0}")]
    Parse(#[from] python_parser::ParseError),

    /// VFS error.
    #[error("VFS error: {0}")]
    Vfs(#[from] vfs_core::VfsError),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Mount error.
    #[error("Mount error: {0}")]
    Mount(String),
}
