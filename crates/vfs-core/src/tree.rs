//! Virtual filesystem tree.

use crate::error::VfsError;
use crate::node::{FileContent, VfsNode};
use std::path::{Component, Path};
use tracing::{debug, error, info, trace, warn};

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

    /// Write content to a file, optionally syncing to the original source file.
    ///
    /// This handles two cases:
    /// 1. Full file write (source_path is set): writes entire content to source file
    /// 2. Fragment write (source_fragment is set): replaces the fragment span in the source file
    ///
    /// Returns the updated source file path if a write occurred (for reload purposes).
    pub fn write(
        &mut self,
        path: impl AsRef<Path>,
        content: Vec<u8>,
        sync_to_source: bool,
    ) -> Result<Option<std::path::PathBuf>, VfsError> {
        let vfs_path = path.as_ref();
        info!(
            path = %vfs_path.display(),
            content_len = content.len(),
            sync_to_source = sync_to_source,
            "VFS write operation started"
        );

        let node = self.get_mut(vfs_path)?;
        if !node.is_file() {
            error!(path = %vfs_path.display(), "Write failed: not a file");
            return Err(VfsError::NotAFile(vfs_path.display().to_string()));
        }

        debug!(
            path = %vfs_path.display(),
            has_source_fragment = node.source_fragment.is_some(),
            has_source_path = node.source_path.is_some(),
            "Node source info"
        );

        let mut modified_source: Option<std::path::PathBuf> = None;
        let mut new_fragment_end: Option<usize> = None;

        // If sync_to_source is enabled, try to write back
        if sync_to_source {
            // First check for source fragment (method/function fragment)
            if let Some(fragment) = &node.source_fragment {
                let source_path = fragment.source_path.clone();
                let start = fragment.start_byte;
                let end = fragment.end_byte;

                info!(
                    source_file = %source_path.display(),
                    start_byte = start,
                    end_byte = end,
                    start_line = fragment.start_line,
                    end_line = fragment.end_line,
                    fragment_size = end - start,
                    new_content_size = content.len(),
                    "Syncing fragment write to source file"
                );

                // Read the original source file
                debug!(source_file = %source_path.display(), "Reading original source file");
                let original_content = std::fs::read(&source_path).map_err(|e| {
                    error!(
                        source_file = %source_path.display(),
                        error = %e,
                        "Failed to read source file"
                    );
                    VfsError::InvalidPath(format!("Failed to read source file: {}", e))
                })?;

                debug!(
                    original_size = original_content.len(),
                    "Original source file read successfully"
                );

                // Validate byte ranges
                if start > original_content.len() || end > original_content.len() {
                    error!(
                        start_byte = start,
                        end_byte = end,
                        file_size = original_content.len(),
                        "Fragment byte range exceeds file size"
                    );
                    return Err(VfsError::InvalidPath(format!(
                        "Fragment byte range {}..{} exceeds file size {}",
                        start, end, original_content.len()
                    )));
                }

                // Build new file content by replacing the fragment span
                let mut new_file_content = Vec::new();
                new_file_content.extend_from_slice(&original_content[..start]);
                new_file_content.extend_from_slice(&content);
                new_file_content.extend_from_slice(&original_content[end..]);

                debug!(
                    before_fragment = start,
                    new_content = content.len(),
                    after_fragment = original_content.len() - end,
                    total_new_size = new_file_content.len(),
                    "Built new file content"
                );

                // Write back to the source file
                info!(
                    source_file = %source_path.display(),
                    new_size = new_file_content.len(),
                    "Writing updated content to source file"
                );
                std::fs::write(&source_path, &new_file_content).map_err(|e| {
                    error!(
                        source_file = %source_path.display(),
                        error = %e,
                        "Failed to write to source file"
                    );
                    VfsError::InvalidPath(format!("Failed to write to source: {}", e))
                })?;

                info!(
                    source_file = %source_path.display(),
                    "Source file updated successfully"
                );
                modified_source = Some(source_path.clone());

                // Calculate new fragment end_byte to update after we're done with the borrow
                // This is critical for subsequent writes to use the correct byte range
                new_fragment_end = Some(start + content.len());
                debug!(
                    old_end_byte = end,
                    new_end_byte = start + content.len(),
                    "Will update fragment end_byte after source write"
                );
            }
            // Fall back to full file source_path
            else if let Some(source_path) = &node.source_path {
                info!(
                    source_file = %source_path.display(),
                    content_size = content.len(),
                    "Syncing full file write to source"
                );
                std::fs::write(source_path, &content).map_err(|e| {
                    error!(
                        source_file = %source_path.display(),
                        error = %e,
                        "Failed to write to source file"
                    );
                    VfsError::InvalidPath(format!("Failed to write to source: {}", e))
                })?;
                info!(
                    source_file = %source_path.display(),
                    "Source file updated successfully"
                );
                modified_source = Some(source_path.clone());
            } else {
                warn!(
                    path = %vfs_path.display(),
                    "sync_to_source enabled but node has no source_fragment or source_path"
                );
            }
        } else {
            debug!(path = %vfs_path.display(), "sync_to_source disabled, only updating in-memory content");
        }

        // Update in-memory content
        debug!(path = %vfs_path.display(), content_len = content.len(), "Updating in-memory content");
        node.set_content(FileContent::from_bytes(content));

        // Update the fragment's end_byte if it changed
        if let Some(new_end) = new_fragment_end {
            if let Some(ref mut frag) = node.source_fragment {
                debug!(
                    old_end = frag.end_byte,
                    new_end = new_end,
                    "Updated fragment end_byte"
                );
                frag.end_byte = new_end;
            }
        }

        info!(
            path = %vfs_path.display(),
            modified_source = ?modified_source,
            "VFS write operation completed"
        );
        Ok(modified_source)
    }

    /// Truncate a file to a given size, optionally syncing to the original source file.
    pub fn truncate(
        &mut self,
        path: impl AsRef<Path>,
        size: u64,
        sync_to_source: bool,
    ) -> Result<Option<std::path::PathBuf>, VfsError> {
        let node = self.get_mut(path.as_ref())?;
        if !node.is_file() {
            return Err(VfsError::NotAFile(path.as_ref().display().to_string()));
        }

        let current_content = node.get_content().unwrap_or_default();
        let new_content = if size == 0 {
            Vec::new()
        } else if (size as usize) < current_content.len() {
            current_content[..size as usize].to_vec()
        } else {
            let mut extended = current_content;
            extended.resize(size as usize, 0);
            extended
        };

        // Use the write method for consistency
        self.write(path, new_content, sync_to_source)
    }

    /// Rename/move a node from one path to another.
    ///
    /// If the destination exists and has source tracking info (source_fragment or source_path),
    /// the source content is updated with the new node's content, and the source tracking
    /// info is preserved on the new node.
    ///
    /// Returns the modified source path if any source file was updated.
    pub fn rename(
        &mut self,
        from_path: impl AsRef<Path>,
        to_path: impl AsRef<Path>,
        sync_to_source: bool,
    ) -> Result<Option<std::path::PathBuf>, VfsError> {
        let from = from_path.as_ref();
        let to = to_path.as_ref();

        info!(
            from = %from.display(),
            to = %to.display(),
            sync_to_source = sync_to_source,
            "VFS rename operation started"
        );

        // Get the source node's content first
        let source_content = self.read(from)?;
        debug!(
            from = %from.display(),
            content_len = source_content.len(),
            "Read source content for rename"
        );

        // Check if destination exists and has source tracking info
        let dest_source_info = if let Ok(dest_node) = self.get(to) {
            let fragment = dest_node.source_fragment.clone();
            let path = dest_node.source_path.clone();
            debug!(
                to = %to.display(),
                has_source_fragment = fragment.is_some(),
                has_source_path = path.is_some(),
                "Destination exists with source tracking info"
            );
            Some((fragment, path))
        } else {
            debug!(to = %to.display(), "Destination does not exist");
            None
        };

        // Remove the source node
        let mut source_node = self.remove(from)?;
        debug!(from = %from.display(), "Removed source node");

        // Update the source node's name to match the destination
        let to_components = Self::normalize_path(to);
        if let Some(new_name) = to_components.last() {
            source_node.name = new_name.clone();
        }

        // If destination had source tracking info, apply it to the source node
        // and sync the content to the original source file
        let mut modified_source: Option<std::path::PathBuf> = None;

        if let Some((dest_fragment, dest_source_path)) = dest_source_info {
            // Remove the existing destination node first
            let _ = self.remove(to);

            // Transfer source tracking info to the moved node
            if let Some(fragment) = dest_fragment {
                info!(
                    source_file = %fragment.source_path.display(),
                    start_byte = fragment.start_byte,
                    end_byte = fragment.end_byte,
                    "Transferring source fragment from destination to renamed node"
                );
                source_node.source_fragment = Some(fragment.clone());

                // If sync is enabled, write the content to the source file
                if sync_to_source {
                    let source_path = fragment.source_path.clone();
                    let start = fragment.start_byte;
                    let end = fragment.end_byte;

                    debug!(source_file = %source_path.display(), "Reading original source file");
                    let original_content = std::fs::read(&source_path).map_err(|e| {
                        error!(
                            source_file = %source_path.display(),
                            error = %e,
                            "Failed to read source file"
                        );
                        VfsError::InvalidPath(format!("Failed to read source file: {}", e))
                    })?;

                    // Validate byte ranges
                    if start > original_content.len() || end > original_content.len() {
                        error!(
                            start_byte = start,
                            end_byte = end,
                            file_size = original_content.len(),
                            "Fragment byte range exceeds file size"
                        );
                        return Err(VfsError::InvalidPath(format!(
                            "Fragment byte range {}..{} exceeds file size {}",
                            start, end, original_content.len()
                        )));
                    }

                    // Build new file content
                    let mut new_file_content = Vec::new();
                    new_file_content.extend_from_slice(&original_content[..start]);
                    new_file_content.extend_from_slice(&source_content);
                    new_file_content.extend_from_slice(&original_content[end..]);

                    info!(
                        source_file = %source_path.display(),
                        new_size = new_file_content.len(),
                        "Writing updated content to source file (via rename)"
                    );
                    std::fs::write(&source_path, &new_file_content).map_err(|e| {
                        error!(
                            source_file = %source_path.display(),
                            error = %e,
                            "Failed to write to source file"
                        );
                        VfsError::InvalidPath(format!("Failed to write to source: {}", e))
                    })?;

                    info!(source_file = %source_path.display(), "Source file updated via rename");
                    modified_source = Some(source_path);
                }
            } else if let Some(src_path) = dest_source_path {
                info!(
                    source_file = %src_path.display(),
                    "Transferring source path from destination to renamed node"
                );
                source_node.source_path = Some(src_path.clone());

                if sync_to_source {
                    std::fs::write(&src_path, &source_content).map_err(|e| {
                        error!(
                            source_file = %src_path.display(),
                            error = %e,
                            "Failed to write to source file"
                        );
                        VfsError::InvalidPath(format!("Failed to write to source: {}", e))
                    })?;
                    info!(source_file = %src_path.display(), "Source file updated via rename");
                    modified_source = Some(src_path);
                }
            }
        } else {
            // Destination didn't exist, just remove it in case (shouldn't happen)
            let _ = self.remove(to);
        }

        // Create parent directories for destination if needed
        let to_parent: std::path::PathBuf = to_components[..to_components.len().saturating_sub(1)]
            .iter()
            .collect();
        if !to_parent.as_os_str().is_empty() {
            self.mkdir_p(&to_parent)?;
        }

        // Add the node at the new location
        let mut current = &mut self.root;
        for component in &to_components[..to_components.len().saturating_sub(1)] {
            current = current.get_child_mut(component).unwrap();
        }
        current.add_child(source_node).map_err(|e| VfsError::InvalidPath(e.to_string()))?;

        info!(
            from = %from.display(),
            to = %to.display(),
            modified_source = ?modified_source,
            "VFS rename operation completed"
        );

        Ok(modified_source)
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
