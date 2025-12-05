//! Virtual filesystem tree.

use crate::error::VfsError;
use crate::node::{FileContent, VfsNode};
use std::path::{Component, Path};
use tracing::trace;

/// A virtual filesystem tree.
#[derive(Debug, Clone)]
pub struct VfsTree {
    root: VfsNode,
}

impl VfsTree {
    /// Create a new empty virtual filesystem.
    pub fn new() -> Self {
        Self {
            root: VfsNode::directory(""),
        }
    }

    /// Get the root node.
    pub fn root(&self) -> &VfsNode {
        &self.root
    }

    /// Get a mutable reference to the root node.
    pub fn root_mut(&mut self) -> &mut VfsNode {
        &mut self.root
    }

    /// Normalize a path to components.
    fn normalize_path(path: &Path) -> Vec<String> {
        path.components()
            .filter_map(|c| match c {
                Component::Normal(s) => s.to_str().map(String::from),
                _ => None,
            })
            .collect()
    }

    /// Get a node by path.
    pub fn get(&self, path: impl AsRef<Path>) -> Result<&VfsNode, VfsError> {
        let components = Self::normalize_path(path.as_ref());

        if components.is_empty() {
            return Ok(&self.root);
        }

        let mut current = &self.root;
        for component in &components {
            current = current
                .get_child(component)
                .ok_or_else(|| VfsError::NotFound(path.as_ref().display().to_string()))?;
        }

        Ok(current)
    }

    /// Get a mutable node by path.
    pub fn get_mut(&mut self, path: impl AsRef<Path>) -> Result<&mut VfsNode, VfsError> {
        let components = Self::normalize_path(path.as_ref());

        if components.is_empty() {
            return Ok(&mut self.root);
        }

        let mut current = &mut self.root;
        for component in components {
            current = current
                .get_child_mut(&component)
                .ok_or_else(|| VfsError::NotFound(path.as_ref().display().to_string()))?;
        }

        Ok(current)
    }

    /// Check if a path exists.
    pub fn exists(&self, path: impl AsRef<Path>) -> bool {
        self.get(path).is_ok()
    }

    /// Create a directory at the given path.
    /// Creates parent directories as needed.
    pub fn mkdir_p(&mut self, path: impl AsRef<Path>) -> Result<(), VfsError> {
        let components = Self::normalize_path(path.as_ref());
        trace!("mkdir_p: {:?}", components);

        let mut current = &mut self.root;
        for component in components {
            if !current.children.contains_key(&component) {
                let new_dir = VfsNode::directory(&component);
                current.add_child(new_dir).unwrap();
            }
            current = current.get_child_mut(&component).unwrap();
            if !current.is_dir() {
                return Err(VfsError::NotADirectory(component));
            }
        }

        Ok(())
    }

    /// Create a file at the given path.
    /// Creates parent directories as needed.
    pub fn create_file(
        &mut self,
        path: impl AsRef<Path>,
        content: FileContent,
    ) -> Result<(), VfsError> {
        let path = path.as_ref();
        let components = Self::normalize_path(path);
        trace!("create_file: {:?}", components);

        if components.is_empty() {
            return Err(VfsError::InvalidPath(path.display().to_string()));
        }

        let (parent_components, file_name) = components.split_at(components.len() - 1);
        let file_name = &file_name[0];

        // Ensure parent directory exists
        if !parent_components.is_empty() {
            let parent_path: std::path::PathBuf = parent_components.iter().collect();
            self.mkdir_p(&parent_path)?;
        }

        // Navigate to parent
        let mut current = &mut self.root;
        for component in parent_components {
            current = current.get_child_mut(component).unwrap();
        }

        // Create the file
        let file_node = VfsNode::file(file_name, content);
        current.add_child(file_node).unwrap();

        Ok(())
    }

    /// Create a file from a string.
    pub fn create_file_from_string(
        &mut self,
        path: impl AsRef<Path>,
        content: impl Into<String>,
    ) -> Result<(), VfsError> {
        self.create_file(path, FileContent::from_string(content))
    }

    /// Remove a node at the given path.
    pub fn remove(&mut self, path: impl AsRef<Path>) -> Result<VfsNode, VfsError> {
        let path = path.as_ref();
        let components = Self::normalize_path(path);

        if components.is_empty() {
            return Err(VfsError::InvalidPath("Cannot remove root".to_string()));
        }

        let (parent_components, node_name) = components.split_at(components.len() - 1);
        let node_name = &node_name[0];

        // Navigate to parent
        let mut current = &mut self.root;
        for component in parent_components {
            current = current
                .get_child_mut(component)
                .ok_or_else(|| VfsError::NotFound(path.display().to_string()))?;
        }

        // Remove the node
        current
            .children
            .remove(node_name)
            .ok_or_else(|| VfsError::NotFound(path.display().to_string()))
    }

    /// List entries in a directory.
    pub fn list(&self, path: impl AsRef<Path>) -> Result<Vec<&str>, VfsError> {
        let node = self.get(path.as_ref())?;
        if !node.is_dir() {
            return Err(VfsError::NotADirectory(
                path.as_ref().display().to_string(),
            ));
        }
        Ok(node.children.keys().map(|s| s.as_str()).collect())
    }

    /// Read file content.
    pub fn read(&self, path: impl AsRef<Path>) -> Result<Vec<u8>, VfsError> {
        let node = self.get(path.as_ref())?;
        if !node.is_file() {
            return Err(VfsError::NotAFile(path.as_ref().display().to_string()));
        }
        Ok(node.get_content().unwrap_or_default())
    }

    /// Read file content as string.
    pub fn read_to_string(&self, path: impl AsRef<Path>) -> Result<String, VfsError> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes).map_err(|_| VfsError::InvalidPath("Invalid UTF-8".to_string()))
    }

    /// Walk all nodes in the tree.
    pub fn walk(&self) -> VfsWalker<'_> {
        VfsWalker::new(&self.root)
    }
}

impl Default for VfsTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Iterator for walking the virtual filesystem tree.
pub struct VfsWalker<'a> {
    stack: Vec<(String, &'a VfsNode)>,
}

impl<'a> VfsWalker<'a> {
    fn new(root: &'a VfsNode) -> Self {
        Self {
            stack: vec![("/".to_string(), root)],
        }
    }
}

impl<'a> Iterator for VfsWalker<'a> {
    type Item = (String, &'a VfsNode);

    fn next(&mut self) -> Option<Self::Item> {
        let (path, node) = self.stack.pop()?;

        // Add children to stack (in reverse order to maintain alphabetical traversal)
        if node.is_dir() {
            let mut children: Vec<_> = node.children.iter().collect();
            children.sort_by(|a, b| b.0.cmp(a.0));
            for (name, child) in children {
                let child_path = if path == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", path, name)
                };
                self.stack.push((child_path, child));
            }
        }

        Some((path, node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_directory() {
        let mut tree = VfsTree::new();
        tree.mkdir_p("a/b/c").unwrap();

        assert!(tree.exists("a"));
        assert!(tree.exists("a/b"));
        assert!(tree.exists("a/b/c"));
    }

    #[test]
    fn test_create_file() {
        let mut tree = VfsTree::new();
        tree.create_file_from_string("a/b/file.txt", "Hello, World!")
            .unwrap();

        assert!(tree.exists("a/b/file.txt"));
        assert_eq!(
            tree.read_to_string("a/b/file.txt").unwrap(),
            "Hello, World!"
        );
    }

    #[test]
    fn test_list_directory() {
        let mut tree = VfsTree::new();
        tree.mkdir_p("dir").unwrap();
        tree.create_file_from_string("dir/a.txt", "a").unwrap();
        tree.create_file_from_string("dir/b.txt", "b").unwrap();
        tree.mkdir_p("dir/subdir").unwrap();

        let entries = tree.list("dir").unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.contains(&"a.txt"));
        assert!(entries.contains(&"b.txt"));
        assert!(entries.contains(&"subdir"));
    }

    #[test]
    fn test_remove() {
        let mut tree = VfsTree::new();
        tree.create_file_from_string("file.txt", "content").unwrap();
        assert!(tree.exists("file.txt"));

        tree.remove("file.txt").unwrap();
        assert!(!tree.exists("file.txt"));
    }

    #[test]
    fn test_walk() {
        let mut tree = VfsTree::new();
        tree.mkdir_p("a/b").unwrap();
        tree.create_file_from_string("a/file.txt", "content")
            .unwrap();

        let paths: Vec<String> = tree.walk().map(|(p, _)| p).collect();
        assert!(paths.contains(&"/".to_string()));
        assert!(paths.contains(&"/a".to_string()));
        assert!(paths.contains(&"/a/b".to_string()));
        assert!(paths.contains(&"/a/file.txt".to_string()));
    }
}
