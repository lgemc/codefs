//! Python code parser for extracting class and method definitions.
//!
//! This library parses Python source files and extracts structural information
//! about classes, methods, and functions, organizing them into a tree structure
//! suitable for virtual filesystem mounting.

mod error;
mod parser;
mod types;

pub use error::ParseError;
pub use parser::PythonParser;
pub use types::{
    ClassDef, Decorator, FunctionDef, MethodDef, ModuleDef, Parameter, ParsedDirectory,
    ParsedFile, Span,
};

/// Re-export for convenience
pub mod prelude {
    pub use crate::{
        ClassDef, Decorator, FunctionDef, MethodDef, ModuleDef, Parameter, ParseError,
        ParsedDirectory, ParsedFile, PythonParser, Span,
    };
}
