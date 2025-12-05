//! Python parser implementation using tree-sitter.

use crate::error::ParseError;
use crate::types::*;
use std::path::Path;
use tracing::{debug, trace, warn};
use tree_sitter::{Node, Parser, Tree};
use walkdir::WalkDir;

/// Parser for Python source code.
pub struct PythonParser {
    parser: Parser,
}

impl PythonParser {
    /// Create a new Python parser.
    pub fn new() -> Result<Self, ParseError> {
        let mut parser = Parser::new();
        let language = tree_sitter_python::LANGUAGE;
        parser
            .set_language(&language.into())
            .map_err(|e| ParseError::ParserInit(e.to_string()))?;
        Ok(Self { parser })
    }

    /// Parse a Python source string.
    pub fn parse_source(&mut self, source: &str, name: &str) -> Result<ModuleDef, ParseError> {
        let tree = self
            .parser
            .parse(source, None)
            .ok_or_else(|| ParseError::SyntaxError {
                path: name.into(),
                message: "Failed to parse".into(),
            })?;

        self.extract_module(&tree, source, name)
    }

    /// Parse a Python file.
    pub fn parse_file(&mut self, path: &Path) -> Result<ParsedFile, ParseError> {
        let source =
            std::fs::read_to_string(path).map_err(|e| ParseError::file_read(path, e))?;

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        let module = self.parse_source(&source, name)?;

        Ok(ParsedFile {
            path: path.to_path_buf(),
            module,
        })
    }

    /// Parse all Python files in a directory recursively.
    pub fn parse_directory(&mut self, path: &Path) -> Result<ParsedDirectory, ParseError> {
        let mut parsed = ParsedDirectory::new(path.to_path_buf());

        for entry in WalkDir::new(path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let file_path = entry.path();
            if file_path.extension().and_then(|s| s.to_str()) == Some("py") {
                debug!("Parsing: {}", file_path.display());
                match self.parse_file(file_path) {
                    Ok(parsed_file) => {
                        let relative = file_path
                            .strip_prefix(path)
                            .unwrap_or(file_path)
                            .to_path_buf();
                        parsed.add_file(relative, parsed_file);
                    }
                    Err(e) => {
                        warn!("Failed to parse {}: {}", file_path.display(), e);
                    }
                }
            }
        }

        Ok(parsed)
    }

    fn extract_module(
        &self,
        tree: &Tree,
        source: &str,
        name: &str,
    ) -> Result<ModuleDef, ParseError> {
        let root = tree.root_node();
        let mut module = ModuleDef {
            name: name.to_string(),
            docstring: None,
            imports: Vec::new(),
            functions: Vec::new(),
            classes: Vec::new(),
            source: source.to_string(),
        };

        let mut cursor = root.walk();
        let children: Vec<_> = root.children(&mut cursor).collect();

        // Check for module docstring (first expression statement with a string)
        if let Some(first) = children.first() {
            if first.kind() == "expression_statement" {
                if let Some(string_node) = first.child(0) {
                    if string_node.kind() == "string" {
                        module.docstring = Some(self.extract_string_content(string_node, source));
                    }
                }
            }
        }

        for child in children {
            match child.kind() {
                "import_statement" | "import_from_statement" => {
                    module.imports.push(self.node_text(&child, source).to_string());
                }
                "function_definition" => {
                    if let Some(func) = self.extract_function(&child, source, &[]) {
                        module.functions.push(func);
                    }
                }
                "decorated_definition" => {
                    let decorators = self.extract_decorators(&child, source);
                    if let Some(definition) = child.child_by_field_name("definition") {
                        match definition.kind() {
                            "function_definition" => {
                                if let Some(func) =
                                    self.extract_function(&definition, source, &decorators)
                                {
                                    module.functions.push(func);
                                }
                            }
                            "class_definition" => {
                                if let Some(class) =
                                    self.extract_class(&definition, source, &decorators)
                                {
                                    module.classes.push(class);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "class_definition" => {
                    if let Some(class) = self.extract_class(&child, source, &[]) {
                        module.classes.push(class);
                    }
                }
                _ => {}
            }
        }

        Ok(module)
    }

    fn extract_decorators(&self, decorated_node: &Node, source: &str) -> Vec<Decorator> {
        let mut decorators = Vec::new();
        let mut cursor = decorated_node.walk();

        for child in decorated_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                let text = self.node_text(&child, source).to_string();
                let name = self.extract_decorator_name(&child, source);
                let span = self.node_span(&child);
                decorators.push(Decorator { text, name, span });
            }
        }

        decorators
    }

    fn extract_decorator_name(&self, decorator: &Node, source: &str) -> String {
        // Skip the '@' and get the name/attribute
        let mut cursor = decorator.walk();
        for child in decorator.children(&mut cursor) {
            match child.kind() {
                "identifier" => return self.node_text(&child, source).to_string(),
                "attribute" => return self.node_text(&child, source).to_string(),
                "call" => {
                    // For @decorator(...), get the function name
                    if let Some(func) = child.child_by_field_name("function") {
                        return self.node_text(&func, source).to_string();
                    }
                }
                _ => {}
            }
        }
        String::new()
    }

    fn extract_function(
        &self,
        node: &Node,
        source: &str,
        decorators: &[Decorator],
    ) -> Option<FunctionDef> {
        let name = node
            .child_by_field_name("name")
            .map(|n| self.node_text(&n, source).to_string())?;

        trace!("Extracting function: {}", name);

        let parameters = self.extract_parameters(node, source);
        let return_type = node
            .child_by_field_name("return_type")
            .map(|n| self.node_text(&n, source).to_string());

        let docstring = self.extract_body_docstring(node, source);

        // Check if async
        let is_async = self.node_text(node, source).starts_with("async ");

        // Calculate the full span including decorators
        let span = if !decorators.is_empty() {
            let dec_start = decorators.first().unwrap().span.start;
            let dec_start_line = decorators.first().unwrap().span.start_line;
            let node_span = self.node_span(node);
            Span::new(dec_start, node_span.end, dec_start_line, node_span.end_line)
        } else {
            self.node_span(node)
        };

        // Get source including decorators
        let full_source = if !decorators.is_empty() {
            let dec_text: String = decorators.iter().map(|d| format!("{}\n", d.text)).collect();
            format!("{}{}", dec_text, self.node_text(node, source))
        } else {
            self.node_text(node, source).to_string()
        };

        Some(FunctionDef {
            name,
            decorators: decorators.to_vec(),
            parameters,
            return_type,
            docstring,
            source: full_source,
            span,
            is_async,
        })
    }

    fn extract_class(
        &self,
        node: &Node,
        source: &str,
        decorators: &[Decorator],
    ) -> Option<ClassDef> {
        let name = node
            .child_by_field_name("name")
            .map(|n| self.node_text(&n, source).to_string())?;

        trace!("Extracting class: {}", name);

        let bases = self.extract_bases(node, source);
        let docstring = self.extract_body_docstring(node, source);

        let mut methods = Vec::new();
        let mut nested_classes = Vec::new();

        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                match child.kind() {
                    "function_definition" => {
                        if let Some(method) = self.extract_method(&child, source, &[]) {
                            methods.push(method);
                        }
                    }
                    "decorated_definition" => {
                        let method_decorators = self.extract_decorators(&child, source);
                        if let Some(definition) = child.child_by_field_name("definition") {
                            if definition.kind() == "function_definition" {
                                if let Some(method) =
                                    self.extract_method(&definition, source, &method_decorators)
                                {
                                    methods.push(method);
                                }
                            } else if definition.kind() == "class_definition" {
                                if let Some(nested) =
                                    self.extract_class(&definition, source, &method_decorators)
                                {
                                    nested_classes.push(nested);
                                }
                            }
                        }
                    }
                    "class_definition" => {
                        if let Some(nested) = self.extract_class(&child, source, &[]) {
                            nested_classes.push(nested);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Calculate the full span including decorators
        let span = if !decorators.is_empty() {
            let dec_start = decorators.first().unwrap().span.start;
            let dec_start_line = decorators.first().unwrap().span.start_line;
            let node_span = self.node_span(node);
            Span::new(dec_start, node_span.end, dec_start_line, node_span.end_line)
        } else {
            self.node_span(node)
        };

        // Get source including decorators
        let full_source = if !decorators.is_empty() {
            let dec_text: String = decorators.iter().map(|d| format!("{}\n", d.text)).collect();
            format!("{}{}", dec_text, self.node_text(node, source))
        } else {
            self.node_text(node, source).to_string()
        };

        Some(ClassDef {
            name,
            bases,
            decorators: decorators.to_vec(),
            docstring,
            methods,
            source: full_source,
            span,
            nested_classes,
        })
    }

    fn extract_method(
        &self,
        node: &Node,
        source: &str,
        decorators: &[Decorator],
    ) -> Option<MethodDef> {
        let name = node
            .child_by_field_name("name")
            .map(|n| self.node_text(&n, source).to_string())?;

        trace!("Extracting method: {}", name);

        let parameters = self.extract_parameters(node, source);
        let return_type = node
            .child_by_field_name("return_type")
            .map(|n| self.node_text(&n, source).to_string());

        let docstring = self.extract_body_docstring(node, source);

        let is_async = self.node_text(node, source).starts_with("async ");

        let is_classmethod = decorators.iter().any(|d| d.name == "classmethod");
        let is_staticmethod = decorators.iter().any(|d| d.name == "staticmethod");
        let is_property = decorators.iter().any(|d| d.name == "property");

        // Calculate the full span including decorators
        let span = if !decorators.is_empty() {
            let dec_start = decorators.first().unwrap().span.start;
            let dec_start_line = decorators.first().unwrap().span.start_line;
            let node_span = self.node_span(node);
            Span::new(dec_start, node_span.end, dec_start_line, node_span.end_line)
        } else {
            self.node_span(node)
        };

        // Get source including decorators
        let full_source = if !decorators.is_empty() {
            let dec_text: String = decorators.iter().map(|d| format!("{}\n", d.text)).collect();
            format!("{}{}", dec_text, self.node_text(node, source))
        } else {
            self.node_text(node, source).to_string()
        };

        Some(MethodDef {
            name,
            decorators: decorators.to_vec(),
            parameters,
            return_type,
            docstring,
            source: full_source,
            span,
            is_async,
            is_classmethod,
            is_staticmethod,
            is_property,
        })
    }

    fn extract_parameters(&self, node: &Node, source: &str) -> Vec<Parameter> {
        let mut params = Vec::new();

        if let Some(params_node) = node.child_by_field_name("parameters") {
            let mut cursor = params_node.walk();
            for child in params_node.children(&mut cursor) {
                match child.kind() {
                    "identifier" => {
                        params.push(Parameter {
                            name: self.node_text(&child, source).to_string(),
                            type_annotation: None,
                            default_value: None,
                            is_args: false,
                            is_kwargs: false,
                        });
                    }
                    "typed_parameter" => {
                        let name = child
                            .child(0)
                            .map(|n| self.node_text(&n, source).to_string())
                            .unwrap_or_default();
                        let type_annotation = child
                            .child_by_field_name("type")
                            .map(|n| self.node_text(&n, source).to_string());
                        params.push(Parameter {
                            name,
                            type_annotation,
                            default_value: None,
                            is_args: false,
                            is_kwargs: false,
                        });
                    }
                    "default_parameter" => {
                        let name = child
                            .child_by_field_name("name")
                            .map(|n| self.node_text(&n, source).to_string())
                            .unwrap_or_default();
                        let default_value = child
                            .child_by_field_name("value")
                            .map(|n| self.node_text(&n, source).to_string());
                        params.push(Parameter {
                            name,
                            type_annotation: None,
                            default_value,
                            is_args: false,
                            is_kwargs: false,
                        });
                    }
                    "typed_default_parameter" => {
                        let name = child
                            .child_by_field_name("name")
                            .map(|n| self.node_text(&n, source).to_string())
                            .unwrap_or_default();
                        let type_annotation = child
                            .child_by_field_name("type")
                            .map(|n| self.node_text(&n, source).to_string());
                        let default_value = child
                            .child_by_field_name("value")
                            .map(|n| self.node_text(&n, source).to_string());
                        params.push(Parameter {
                            name,
                            type_annotation,
                            default_value,
                            is_args: false,
                            is_kwargs: false,
                        });
                    }
                    "list_splat_pattern" => {
                        let name = child
                            .child(0)
                            .map(|n| self.node_text(&n, source).to_string())
                            .unwrap_or_else(|| "args".to_string());
                        params.push(Parameter {
                            name,
                            type_annotation: None,
                            default_value: None,
                            is_args: true,
                            is_kwargs: false,
                        });
                    }
                    "dictionary_splat_pattern" => {
                        let name = child
                            .child(0)
                            .map(|n| self.node_text(&n, source).to_string())
                            .unwrap_or_else(|| "kwargs".to_string());
                        params.push(Parameter {
                            name,
                            type_annotation: None,
                            default_value: None,
                            is_args: false,
                            is_kwargs: true,
                        });
                    }
                    _ => {}
                }
            }
        }

        params
    }

    fn extract_bases(&self, node: &Node, source: &str) -> Vec<String> {
        let mut bases = Vec::new();

        if let Some(superclasses) = node.child_by_field_name("superclasses") {
            let mut cursor = superclasses.walk();
            for child in superclasses.children(&mut cursor) {
                match child.kind() {
                    "identifier" | "attribute" => {
                        bases.push(self.node_text(&child, source).to_string());
                    }
                    "argument_list" => {
                        // Handle class Foo(Bar, Baz)
                        let mut arg_cursor = child.walk();
                        for arg in child.children(&mut arg_cursor) {
                            if arg.kind() == "identifier" || arg.kind() == "attribute" {
                                bases.push(self.node_text(&arg, source).to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        bases
    }

    fn extract_body_docstring(&self, node: &Node, source: &str) -> Option<String> {
        let body = node.child_by_field_name("body")?;
        let mut cursor = body.walk();
        let first_child = body.children(&mut cursor).next()?;

        if first_child.kind() == "expression_statement" {
            if let Some(string_node) = first_child.child(0) {
                if string_node.kind() == "string" {
                    return Some(self.extract_string_content(string_node, source));
                }
            }
        }

        None
    }

    fn extract_string_content(&self, string_node: Node, source: &str) -> String {
        let text = self.node_text(&string_node, source);
        // Remove quotes (handles """, ''', ", ')
        let trimmed = text
            .trim_start_matches("\"\"\"")
            .trim_start_matches("'''")
            .trim_start_matches('"')
            .trim_start_matches('\'')
            .trim_end_matches("\"\"\"")
            .trim_end_matches("'''")
            .trim_end_matches('"')
            .trim_end_matches('\'');
        trimmed.to_string()
    }

    fn node_text<'a>(&self, node: &Node, source: &'a str) -> &'a str {
        &source[node.start_byte()..node.end_byte()]
    }

    fn node_span(&self, node: &Node) -> Span {
        Span::new(
            node.start_byte(),
            node.end_byte(),
            node.start_position().row + 1,
            node.end_position().row + 1,
        )
    }
}

impl Default for PythonParser {
    fn default() -> Self {
        Self::new().expect("Failed to create default parser")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_function() {
        let mut parser = PythonParser::new().unwrap();
        let source = r#"
def hello(name: str) -> str:
    """Say hello to someone."""
    return f"Hello, {name}!"
"#;
        let module = parser.parse_source(source, "test").unwrap();
        assert_eq!(module.functions.len(), 1);
        let func = &module.functions[0];
        assert_eq!(func.name, "hello");
        assert_eq!(func.parameters.len(), 1);
        assert_eq!(func.parameters[0].name, "name");
        assert_eq!(
            func.parameters[0].type_annotation,
            Some("str".to_string())
        );
        assert_eq!(func.return_type, Some("str".to_string()));
        assert!(func.docstring.as_ref().unwrap().contains("Say hello"));
    }

    #[test]
    fn test_parse_class_with_methods() {
        let mut parser = PythonParser::new().unwrap();
        let source = r#"
class Calculator:
    """A simple calculator class."""

    def __init__(self, initial: int = 0):
        self.value = initial

    def add(self, x: int) -> int:
        """Add a number."""
        self.value += x
        return self.value

    @staticmethod
    def multiply(a: int, b: int) -> int:
        return a * b
"#;
        let module = parser.parse_source(source, "test").unwrap();
        assert_eq!(module.classes.len(), 1);
        let class = &module.classes[0];
        assert_eq!(class.name, "Calculator");
        assert!(class.docstring.as_ref().unwrap().contains("simple calculator"));
        assert_eq!(class.methods.len(), 3);

        let init = &class.methods[0];
        assert_eq!(init.name, "__init__");

        let add = &class.methods[1];
        assert_eq!(add.name, "add");
        assert!(add.docstring.as_ref().unwrap().contains("Add a number"));

        let multiply = &class.methods[2];
        assert_eq!(multiply.name, "multiply");
        assert!(multiply.is_staticmethod);
    }

    #[test]
    fn test_parse_decorated_class() {
        let mut parser = PythonParser::new().unwrap();
        let source = r#"
@dataclass
class Point:
    x: int
    y: int
"#;
        let module = parser.parse_source(source, "test").unwrap();
        assert_eq!(module.classes.len(), 1);
        let class = &module.classes[0];
        assert_eq!(class.name, "Point");
        assert_eq!(class.decorators.len(), 1);
        assert_eq!(class.decorators[0].name, "dataclass");
    }

    #[test]
    fn test_parse_async_function() {
        let mut parser = PythonParser::new().unwrap();
        let source = r#"
async def fetch_data(url: str) -> bytes:
    """Fetch data from a URL."""
    pass
"#;
        let module = parser.parse_source(source, "test").unwrap();
        assert_eq!(module.functions.len(), 1);
        let func = &module.functions[0];
        assert!(func.is_async);
    }

    #[test]
    fn test_parse_inheritance() {
        let mut parser = PythonParser::new().unwrap();
        let source = r#"
class Child(Parent, Mixin):
    pass
"#;
        let module = parser.parse_source(source, "test").unwrap();
        assert_eq!(module.classes.len(), 1);
        let class = &module.classes[0];
        assert_eq!(class.bases.len(), 2);
        assert!(class.bases.contains(&"Parent".to_string()));
        assert!(class.bases.contains(&"Mixin".to_string()));
    }
}
