//! Error types for the Python parser.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during Python parsing.
#[derive(Error, Debug)]
pub enum ParseError {
    /// Failed to read a file.
    #[error("Failed to read file '{path}': {source}")]
    FileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to parse Python syntax.
    #[error("Failed to parse Python syntax in '{path}': {message}")]
    SyntaxError { path: PathBuf, message: String },

    /// Failed to initialize the parser.
    #[error("Failed to initialize tree-sitter parser: {0}")]
    ParserInit(String),

    /// Failed to walk directory.
    #[error("Failed to walk directory '{path}': {source}")]
    DirectoryWalk {
        path: PathBuf,
        #[source]
        source: walkdir::Error,
    },

    /// Invalid UTF-8 in source file.
    #[error("Invalid UTF-8 in file '{path}'")]
    InvalidUtf8 { path: PathBuf },
}

impl ParseError {
    pub fn file_read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::FileRead {
            path: path.into(),
            source,
        }
    }

    pub fn syntax_error(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::SyntaxError {
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn directory_walk(path: impl Into<PathBuf>, source: walkdir::Error) -> Self {
        Self::DirectoryWalk {
            path: path.into(),
            source,
        }
    }
}
