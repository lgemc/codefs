//! Type definitions for parsed Python structures.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A span in the source code (start and end byte offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Start byte offset (inclusive).
    pub start: usize,
    /// End byte offset (exclusive).
    pub end: usize,
    /// Start line number (1-indexed).
    pub start_line: usize,
    /// End line number (1-indexed).
    pub end_line: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, start_line: usize, end_line: usize) -> Self {
        Self {
            start,
            end,
            start_line,
            end_line,
        }
    }

    /// Extract the source code for this span.
    pub fn extract<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }
}

/// A decorator applied to a class or function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decorator {
    /// The full decorator text (e.g., "@staticmethod" or "@app.route('/home')").
    pub text: String,
    /// The decorator name (e.g., "staticmethod" or "app.route").
    pub name: String,
    /// Span in source code.
    pub span: Span,
}

/// A function parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    /// Parameter name.
    pub name: String,
    /// Type annotation, if present.
    pub type_annotation: Option<String>,
    /// Default value, if present.
    pub default_value: Option<String>,
    /// Whether this is *args.
    pub is_args: bool,
    /// Whether this is **kwargs.
    pub is_kwargs: bool,
}

/// A standalone function definition (not inside a class).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionDef {
    /// Function name.
    pub name: String,
    /// Decorators applied to this function.
    pub decorators: Vec<Decorator>,
    /// Function parameters.
    pub parameters: Vec<Parameter>,
    /// Return type annotation, if present.
    pub return_type: Option<String>,
    /// The docstring, if present.
    pub docstring: Option<String>,
    /// The full source code of this function.
    pub source: String,
    /// Span in the original source file.
    pub span: Span,
    /// Whether this is an async function.
    pub is_async: bool,
}

/// A method definition inside a class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodDef {
    /// Method name.
    pub name: String,
    /// Decorators applied to this method.
    pub decorators: Vec<Decorator>,
    /// Method parameters (including self/cls).
    pub parameters: Vec<Parameter>,
    /// Return type annotation, if present.
    pub return_type: Option<String>,
    /// The docstring, if present.
    pub docstring: Option<String>,
    /// The full source code of this method.
    pub source: String,
    /// Span in the original source file.
    pub span: Span,
    /// Whether this is an async method.
    pub is_async: bool,
    /// Whether this is a class method (@classmethod).
    pub is_classmethod: bool,
    /// Whether this is a static method (@staticmethod).
    pub is_staticmethod: bool,
    /// Whether this is a property (@property).
    pub is_property: bool,
}

/// A class definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassDef {
    /// Class name.
    pub name: String,
    /// Base classes (inheritance).
    pub bases: Vec<String>,
    /// Decorators applied to this class.
    pub decorators: Vec<Decorator>,
    /// The docstring, if present.
    pub docstring: Option<String>,
    /// Methods defined in this class.
    pub methods: Vec<MethodDef>,
    /// The full source code of this class.
    pub source: String,
    /// Span in the original source file.
    pub span: Span,
    /// Nested classes.
    pub nested_classes: Vec<ClassDef>,
}

/// A parsed Python module (file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDef {
    /// Module name (derived from filename).
    pub name: String,
    /// The module docstring, if present.
    pub docstring: Option<String>,
    /// Top-level imports.
    pub imports: Vec<String>,
    /// Top-level functions.
    pub functions: Vec<FunctionDef>,
    /// Top-level classes.
    pub classes: Vec<ClassDef>,
    /// The full source code.
    pub source: String,
}

/// A parsed Python file with metadata.
#[derive(Debug, Clone)]
pub struct ParsedFile {
    /// Path to the original file.
    pub path: PathBuf,
    /// The parsed module definition.
    pub module: ModuleDef,
}

/// A parsed directory containing Python files.
#[derive(Debug, Clone)]
pub struct ParsedDirectory {
    /// Root path of the directory.
    pub root: PathBuf,
    /// Parsed files indexed by relative path.
    pub files: HashMap<PathBuf, ParsedFile>,
}

impl ParsedDirectory {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: HashMap::new(),
        }
    }

    pub fn add_file(&mut self, relative_path: PathBuf, file: ParsedFile) {
        self.files.insert(relative_path, file);
    }

    /// Get all classes across all files.
    pub fn all_classes(&self) -> impl Iterator<Item = (&PathBuf, &ClassDef)> {
        self.files
            .iter()
            .flat_map(|(path, file)| file.module.classes.iter().map(move |c| (path, c)))
    }

    /// Get all top-level functions across all files.
    pub fn all_functions(&self) -> impl Iterator<Item = (&PathBuf, &FunctionDef)> {
        self.files
            .iter()
            .flat_map(|(path, file)| file.module.functions.iter().map(move |f| (path, f)))
    }
}
