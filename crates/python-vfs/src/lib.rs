//! FUSE filesystem for Python code organized by class and method.
//!
//! This library provides a FUSE filesystem that mounts Python code
//! organized by class and method, allowing navigation through the
//! code structure using standard filesystem operations.

mod builder;
mod error;
mod fuse_fs;

pub use builder::PythonVfsBuilder;
pub use error::PythonVfsError;
pub use fuse_fs::PythonFuseFs;

/// Re-export for convenience.
pub mod prelude {
    pub use crate::{PythonFuseFs, PythonVfsBuilder, PythonVfsError};
}
