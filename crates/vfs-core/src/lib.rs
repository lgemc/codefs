//! Core virtual filesystem abstractions.
//!
//! This library provides an in-memory virtual filesystem that can be used
//! to organize and expose data through a filesystem interface.

mod error;
pub mod logging;
mod node;
mod tree;

pub use error::VfsError;
pub use node::{FileContent, NodeKind, SourceFragment, VfsNode};
pub use tree::VfsTree;

/// Re-export for convenience.
pub mod prelude {
    pub use crate::{FileContent, NodeKind, SourceFragment, VfsError, VfsNode, VfsTree};
}
