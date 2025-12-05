//! Virtual filesystem node types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// The kind of a filesystem node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink { target: String },
}

/// Content of a virtual file.
#[derive(Clone)]
pub enum FileContent {
    /// Static content stored in memory.
    Static(Vec<u8>),
    /// Lazily computed content.
    Lazy(std::sync::Arc<dyn Fn() -> Vec<u8> + Send + Sync>),
}

impl std::fmt::Debug for FileContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(bytes) => f.debug_tuple("Static").field(&bytes.len()).finish(),
            Self::Lazy(_) => f.debug_tuple("Lazy").field(&"<closure>").finish(),
        }
    }
}

impl FileContent {
    /// Create static content from a string.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self::Static(s.into().into_bytes())
    }

    /// Create static content from bytes.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Static(bytes.into())
    }

    /// Create lazy content.
    pub fn lazy<F>(f: F) -> Self
    where
        F: Fn() -> Vec<u8> + Send + Sync + 'static,
    {
        Self::Lazy(std::sync::Arc::new(f))
    }

    /// Get the content as bytes.
    pub fn as_bytes(&self) -> Vec<u8> {
        match self {
            Self::Static(bytes) => bytes.clone(),
            Self::Lazy(f) => f(),
        }
    }

    /// Get the content length.
    pub fn len(&self) -> usize {
        match self {
            Self::Static(bytes) => bytes.len(),
            Self::Lazy(f) => f().len(),
        }
    }

    /// Check if the content is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for FileContent {
    fn default() -> Self {
        Self::Static(Vec::new())
    }
}

/// A node in the virtual filesystem.
#[derive(Debug, Clone)]
pub struct VfsNode {
    /// The name of this node.
    pub name: String,
    /// The kind of this node.
    pub kind: NodeKind,
    /// File content (only for files).
    pub content: Option<FileContent>,
    /// Child nodes (only for directories).
    pub children: HashMap<String, VfsNode>,
    /// Creation time.
    pub created: SystemTime,
    /// Modification time.
    pub modified: SystemTime,
    /// Custom metadata.
    pub metadata: HashMap<String, String>,
}

impl VfsNode {
    /// Create a new directory node.
    pub fn directory(name: impl Into<String>) -> Self {
        let now = SystemTime::now();
        Self {
            name: name.into(),
            kind: NodeKind::Directory,
            content: None,
            children: HashMap::new(),
            created: now,
            modified: now,
            metadata: HashMap::new(),
        }
    }

    /// Create a new file node with content.
    pub fn file(name: impl Into<String>, content: FileContent) -> Self {
        let now = SystemTime::now();
        Self {
            name: name.into(),
            kind: NodeKind::File,
            content: Some(content),
            children: HashMap::new(),
            created: now,
            modified: now,
            metadata: HashMap::new(),
        }
    }

    /// Create a new file node from a string.
    pub fn file_from_string(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self::file(name, FileContent::from_string(content))
    }

    /// Create a new symlink node.
    pub fn symlink(name: impl Into<String>, target: impl Into<String>) -> Self {
        let now = SystemTime::now();
        Self {
            name: name.into(),
            kind: NodeKind::Symlink {
                target: target.into(),
            },
            content: None,
            children: HashMap::new(),
            created: now,
            modified: now,
            metadata: HashMap::new(),
        }
    }

    /// Check if this node is a directory.
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Directory)
    }

    /// Check if this node is a file.
    pub fn is_file(&self) -> bool {
        matches!(self.kind, NodeKind::File)
    }

    /// Check if this node is a symlink.
    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, NodeKind::Symlink { .. })
    }

    /// Add a child node to this directory.
    pub fn add_child(&mut self, child: VfsNode) -> Result<(), &'static str> {
        if !self.is_dir() {
            return Err("Cannot add child to non-directory");
        }
        self.children.insert(child.name.clone(), child);
        self.modified = SystemTime::now();
        Ok(())
    }

    /// Get a child node by name.
    pub fn get_child(&self, name: &str) -> Option<&VfsNode> {
        self.children.get(name)
    }

    /// Get a mutable child node by name.
    pub fn get_child_mut(&mut self, name: &str) -> Option<&mut VfsNode> {
        self.children.get_mut(name)
    }

    /// Get the file content.
    pub fn get_content(&self) -> Option<Vec<u8>> {
        self.content.as_ref().map(|c| c.as_bytes())
    }

    /// Get the file size.
    pub fn size(&self) -> u64 {
        self.content.as_ref().map(|c| c.len() as u64).unwrap_or(0)
    }

    /// Set metadata.
    pub fn set_metadata(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }

    /// Get metadata.
    pub fn get_metadata(&self, key: &str) -> Option<&String> {
        self.metadata.get(key)
    }
}
