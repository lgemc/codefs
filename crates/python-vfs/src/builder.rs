//! Builder for constructing the Python VFS tree.

use crate::error::PythonVfsError;
use python_parser::{ClassDef, FunctionDef, MethodDef, ParsedDirectory, PythonParser, Span};
use std::path::Path;
use tracing::{debug, info};
use vfs_core::{FileContent, SourceFragment, VfsTree};

/// Builder for creating a VFS tree from Python source code.
pub struct PythonVfsBuilder {
    /// Include module-level functions in the tree.
    include_functions: bool,
    /// Include nested classes.
    include_nested_classes: bool,
    /// Add metadata files (.info.txt) for classes and methods.
    include_metadata: bool,
    /// Group by source file first, then by class.
    group_by_file: bool,
}

impl Default for PythonVfsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PythonVfsBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            include_functions: true,
            include_nested_classes: true,
            include_metadata: true,
            group_by_file: true,
        }
    }

    /// Set whether to include module-level functions.
    pub fn include_functions(mut self, include: bool) -> Self {
        self.include_functions = include;
        self
    }

    /// Set whether to include nested classes.
    pub fn include_nested_classes(mut self, include: bool) -> Self {
        self.include_nested_classes = include;
        self
    }

    /// Set whether to include metadata files.
    pub fn include_metadata(mut self, include: bool) -> Self {
        self.include_metadata = include;
        self
    }

    /// Set whether to group by file first.
    pub fn group_by_file(mut self, group: bool) -> Self {
        self.group_by_file = group;
        self
    }

    /// Build a VFS tree from a directory of Python files.
    pub fn build_from_directory(&self, path: &Path) -> Result<VfsTree, PythonVfsError> {
        let mut parser = PythonParser::new()?;
        let parsed = parser.parse_directory(path)?;
        self.build_from_parsed(&parsed)
    }

    /// Build a VFS tree from already-parsed Python code.
    pub fn build_from_parsed(&self, parsed: &ParsedDirectory) -> Result<VfsTree, PythonVfsError> {
        let mut tree = VfsTree::new();

        info!(
            "Building VFS tree from {} files",
            parsed.files.len()
        );

        if self.group_by_file {
            self.build_grouped_by_file(&mut tree, parsed)?;
        } else {
            self.build_flat(&mut tree, parsed)?;
        }

        Ok(tree)
    }

    /// Build tree grouped by source file, then class, then method.
    /// Structure: /<module>/<class>/<method>.py
    fn build_grouped_by_file(
        &self,
        tree: &mut VfsTree,
        parsed: &ParsedDirectory,
    ) -> Result<(), PythonVfsError> {
        for (rel_path, parsed_file) in &parsed.files {
            let module_name = &parsed_file.module.name;
            debug!("Processing module: {}", module_name);

            // Create module directory
            let module_dir = rel_path.with_extension("");
            tree.mkdir_p(&module_dir)?;

            // Get absolute path to original source file
            // parsed_file.path is already the full path from the parser
            let original_path = parsed_file.path.canonicalize().unwrap_or_else(|_| parsed_file.path.clone());

            // Add module docstring if present
            if self.include_metadata {
                if let Some(docstring) = &parsed_file.module.docstring {
                    let info_path = module_dir.join("__module__.txt");
                    tree.create_file_from_string(&info_path, docstring)?;
                }

                // Add full module source with source_path for write-back support
                let vfs_source_path = module_dir.join("__source__.py");
                tree.create_file(
                    &vfs_source_path,
                    FileContent::from_string(&parsed_file.module.source),
                )?;
                // Set the source path for write-back
                if let Ok(node) = tree.get_mut(&vfs_source_path) {
                    node.set_source_path(&original_path);
                }
            }

            // Add top-level functions with source fragment tracking
            if self.include_functions {
                self.add_functions(
                    tree,
                    &module_dir,
                    &parsed_file.module.functions,
                    Some(&original_path),
                )?;
            }

            // Add classes with source fragment tracking
            for class in &parsed_file.module.classes {
                self.add_class(tree, &module_dir, class, Some(&original_path))?;
            }
        }

        Ok(())
    }

    /// Build flat tree with all classes at top level.
    /// Structure: /<class>/<method>.py
    fn build_flat(
        &self,
        tree: &mut VfsTree,
        parsed: &ParsedDirectory,
    ) -> Result<(), PythonVfsError> {
        // Create a __functions__ directory for top-level functions
        if self.include_functions {
            let functions_dir = Path::new("__functions__");
            tree.mkdir_p(functions_dir)?;

            for (_, parsed_file) in &parsed.files {
                let original_path = parsed_file.path.canonicalize().unwrap_or_else(|_| parsed_file.path.clone());
                self.add_functions(
                    tree,
                    functions_dir,
                    &parsed_file.module.functions,
                    Some(&original_path),
                )?;
            }
        }

        // Add all classes at the top level
        for (_, parsed_file) in &parsed.files {
            let original_path = parsed_file.path.canonicalize().unwrap_or_else(|_| parsed_file.path.clone());
            for class in &parsed_file.module.classes {
                self.add_class(tree, Path::new(""), class, Some(&original_path))?;
            }
        }

        Ok(())
    }

    /// Helper to create a SourceFragment from a Span
    fn span_to_fragment(source_file: &Path, span: &Span) -> SourceFragment {
        SourceFragment::new(
            source_file,
            span.start,
            span.end,
            span.start_line,
            span.end_line,
        )
    }

    fn add_functions(
        &self,
        tree: &mut VfsTree,
        base_path: &Path,
        functions: &[FunctionDef],
        source_file: Option<&Path>,
    ) -> Result<(), PythonVfsError> {
        if functions.is_empty() {
            return Ok(());
        }

        let functions_dir = base_path.join("__functions__");
        tree.mkdir_p(&functions_dir)?;

        for func in functions {
            let func_path = functions_dir.join(format!("{}.py", func.name));
            tree.create_file(&func_path, FileContent::from_string(&func.source))?;

            // Set source fragment for write-back support
            if let Some(src_file) = source_file {
                if let Ok(node) = tree.get_mut(&func_path) {
                    node.set_source_fragment(Self::span_to_fragment(src_file, &func.span));
                }
            }

            if self.include_metadata {
                if let Some(docstring) = &func.docstring {
                    let info_path = functions_dir.join(format!("{}.txt", func.name));
                    tree.create_file_from_string(&info_path, docstring)?;
                }
            }
        }

        Ok(())
    }

    fn add_class(
        &self,
        tree: &mut VfsTree,
        base_path: &Path,
        class: &ClassDef,
        source_file: Option<&Path>,
    ) -> Result<(), PythonVfsError> {
        let class_dir = base_path.join(&class.name);
        debug!("Adding class: {} at {:?}", class.name, class_dir);
        tree.mkdir_p(&class_dir)?;

        // Add class metadata
        if self.include_metadata {
            let info = self.build_class_info(class);
            let info_path = class_dir.join("__class__.txt");
            tree.create_file_from_string(&info_path, info)?;

            // Add full class source with fragment tracking
            let source_path = class_dir.join("__source__.py");
            tree.create_file_from_string(&source_path, &class.source)?;

            // Set source fragment for class __source__.py
            if let Some(src_file) = source_file {
                if let Ok(node) = tree.get_mut(&source_path) {
                    node.set_source_fragment(Self::span_to_fragment(src_file, &class.span));
                }
            }
        }

        // Add methods with source fragment tracking
        for method in &class.methods {
            self.add_method(tree, &class_dir, method, source_file)?;
        }

        // Add nested classes
        if self.include_nested_classes {
            for nested in &class.nested_classes {
                self.add_class(tree, &class_dir, nested, source_file)?;
            }
        }

        Ok(())
    }

    fn add_method(
        &self,
        tree: &mut VfsTree,
        class_dir: &Path,
        method: &MethodDef,
        source_file: Option<&Path>,
    ) -> Result<(), PythonVfsError> {
        let method_path = class_dir.join(format!("{}.py", method.name));
        tree.create_file(&method_path, FileContent::from_string(&method.source))?;

        // Set source fragment for write-back support
        if let Some(src_file) = source_file {
            if let Ok(node) = tree.get_mut(&method_path) {
                node.set_source_fragment(Self::span_to_fragment(src_file, &method.span));
            }
        }

        if self.include_metadata {
            if let Some(docstring) = &method.docstring {
                let info_path = class_dir.join(format!("{}.txt", method.name));
                tree.create_file_from_string(&info_path, docstring)?;
            }
        }

        Ok(())
    }

    fn build_class_info(&self, class: &ClassDef) -> String {
        let mut info = String::new();

        info.push_str(&format!("Class: {}\n", class.name));

        if !class.bases.is_empty() {
            info.push_str(&format!("Inherits from: {}\n", class.bases.join(", ")));
        }

        if !class.decorators.is_empty() {
            info.push_str("Decorators:\n");
            for dec in &class.decorators {
                info.push_str(&format!("  {}\n", dec.text));
            }
        }

        if let Some(docstring) = &class.docstring {
            info.push_str(&format!("\nDocstring:\n{}\n", docstring));
        }

        info.push_str(&format!("\nMethods ({}):\n", class.methods.len()));
        for method in &class.methods {
            let decorators: Vec<_> = method.decorators.iter().map(|d| d.name.as_str()).collect();
            let dec_str = if decorators.is_empty() {
                String::new()
            } else {
                format!(" [{}]", decorators.join(", "))
            };
            info.push_str(&format!("  - {}{}\n", method.name, dec_str));
        }

        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use python_parser::PythonParser;

    #[test]
    fn test_build_simple_class() {
        let mut parser = PythonParser::new().unwrap();
        let source = r#"
class Calculator:
    """A simple calculator."""

    def add(self, a: int, b: int) -> int:
        """Add two numbers."""
        return a + b

    def subtract(self, a: int, b: int) -> int:
        """Subtract two numbers."""
        return a - b
"#;
        let module = parser.parse_source(source, "calculator").unwrap();

        let mut parsed = python_parser::ParsedDirectory::new(std::path::PathBuf::from("."));
        parsed.add_file(
            std::path::PathBuf::from("calculator.py"),
            python_parser::ParsedFile {
                path: std::path::PathBuf::from("calculator.py"),
                module,
            },
        );

        let builder = PythonVfsBuilder::new();
        let tree = builder.build_from_parsed(&parsed).unwrap();

        // Check structure
        assert!(tree.exists("calculator/Calculator"));
        assert!(tree.exists("calculator/Calculator/add.py"));
        assert!(tree.exists("calculator/Calculator/subtract.py"));
        assert!(tree.exists("calculator/Calculator/__class__.txt"));
        assert!(tree.exists("calculator/Calculator/__source__.py"));

        // Check content
        let add_content = tree.read_to_string("calculator/Calculator/add.py").unwrap();
        assert!(add_content.contains("def add"));
    }
}
