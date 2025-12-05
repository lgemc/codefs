//! FUSE filesystem implementation.

use crate::error::PythonVfsError;
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use libc::{ENOENT, ENOTDIR, EROFS};
use python_parser::PythonParser;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, trace, warn};
use vfs_core::{FileContent, NodeKind, SourceFragment, VfsNode, VfsTree};

const TTL: Duration = Duration::from_secs(1);
const BLOCK_SIZE: u32 = 512;

/// Inode mapping for the FUSE filesystem.
struct InodeMap {
    /// Path to inode mapping.
    path_to_inode: HashMap<PathBuf, u64>,
    /// Inode to path mapping.
    inode_to_path: HashMap<u64, PathBuf>,
    /// Next available inode number.
    next_inode: u64,
}

impl InodeMap {
    fn new() -> Self {
        let mut map = Self {
            path_to_inode: HashMap::new(),
            inode_to_path: HashMap::new(),
            next_inode: 2, // 1 is reserved for root
        };
        // Root directory
        map.path_to_inode.insert(PathBuf::from("/"), 1);
        map.inode_to_path.insert(1, PathBuf::from("/"));
        map
    }

    fn get_or_create_inode(&mut self, path: &Path) -> u64 {
        if let Some(&inode) = self.path_to_inode.get(path) {
            return inode;
        }

        let inode = self.next_inode;
        self.next_inode += 1;
        self.path_to_inode.insert(path.to_path_buf(), inode);
        self.inode_to_path.insert(inode, path.to_path_buf());
        inode
    }

    fn get_path(&self, inode: u64) -> Option<&PathBuf> {
        self.inode_to_path.get(&inode)
    }

    #[allow(dead_code)]
    fn get_inode(&self, path: &Path) -> Option<u64> {
        self.path_to_inode.get(path).copied()
    }
}

/// FUSE filesystem for Python code.
pub struct PythonFuseFs {
    tree: Arc<RwLock<VfsTree>>,
    inodes: Arc<RwLock<InodeMap>>,
    uid: u32,
    gid: u32,
    /// Whether to sync writes back to original source files.
    sync_writes: bool,
    /// Root directory of the source files (for reloading after writes).
    source_root: Option<PathBuf>,
    /// Saved source fragments for paths that were renamed away.
    /// When vim saves, it renames original.py -> original.py~ (backup), then creates new original.py.
    /// We save the source_fragment here when the original is renamed away, and restore it when
    /// content is written to that path again.
    saved_fragments: Arc<RwLock<HashMap<PathBuf, SourceFragment>>>,
    /// Source files that need to be reloaded after the current file operation completes.
    /// We defer reloads to release() to avoid corrupting fragment offsets during multi-write operations.
    pending_reloads: Arc<RwLock<std::collections::HashSet<PathBuf>>>,
}

impl PythonFuseFs {
    /// Create a new Python FUSE filesystem (read-only by default).
    pub fn new(tree: VfsTree) -> Self {
        Self::with_options(tree, false, None)
    }

    /// Create a new Python FUSE filesystem with write support.
    pub fn with_options(tree: VfsTree, sync_writes: bool, source_root: Option<PathBuf>) -> Self {
        let mut inodes = InodeMap::new();

        // Pre-populate inodes for all nodes
        for (path, _) in tree.walk() {
            inodes.get_or_create_inode(Path::new(&path));
        }

        Self {
            tree: Arc::new(RwLock::new(tree)),
            inodes: Arc::new(RwLock::new(inodes)),
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            sync_writes,
            source_root,
            saved_fragments: Arc::new(RwLock::new(HashMap::new())),
            pending_reloads: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    /// Reload a single Python source file and update all related nodes in the VFS.
    /// This is called after a fragment write to refresh all fragments from that file.
    fn reload_source_file(&self, source_path: &Path) {
        info!("Reloading source file: {:?}", source_path);

        let Some(source_root) = &self.source_root else {
            warn!("No source root configured, cannot reload");
            return;
        };

        // Parse the modified source file
        let mut parser = match PythonParser::new() {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to create parser for reload: {:?}", e);
                return;
            }
        };

        // Read the updated source content
        let source_content = match std::fs::read_to_string(source_path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read source file for reload: {:?}", e);
                return;
            }
        };

        // Get module name from path
        let module_name = source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        let module = match parser.parse_source(&source_content, module_name) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to parse source file for reload: {:?}", e);
                return;
            }
        };

        // Compute the relative path from source_root
        let rel_path = match source_path.strip_prefix(source_root) {
            Ok(p) => p.to_path_buf(),
            Err(_) => {
                warn!("Source path not under source root");
                return;
            }
        };

        let module_dir = rel_path.with_extension("");

        // Update the VFS tree
        let mut tree = self.tree.write().unwrap();

        // Update the module's __source__.py
        let source_vfs_path = module_dir.join("__source__.py");
        if let Ok(node) = tree.get_mut(&source_vfs_path) {
            node.set_content(FileContent::from_string(&module.source));
            // Keep the existing source_path
        }

        // Update each class and its methods
        for class in &module.classes {
            let class_dir = module_dir.join(&class.name);

            // Update class __source__.py
            let class_source_path = class_dir.join("__source__.py");
            if let Ok(node) = tree.get_mut(&class_source_path) {
                node.set_content(FileContent::from_string(&class.source));
                // Update the span
                node.set_source_fragment(SourceFragment::new(
                    source_path,
                    class.span.start,
                    class.span.end,
                    class.span.start_line,
                    class.span.end_line,
                ));
            }

            // Update each method
            for method in &class.methods {
                let method_path = class_dir.join(format!("{}.py", method.name));
                if let Ok(node) = tree.get_mut(&method_path) {
                    node.set_content(FileContent::from_string(&method.source));
                    // Update the span
                    node.set_source_fragment(SourceFragment::new(
                        source_path,
                        method.span.start,
                        method.span.end,
                        method.span.start_line,
                        method.span.end_line,
                    ));
                }
            }
        }

        // Update top-level functions
        let functions_dir = module_dir.join("__functions__");
        for func in &module.functions {
            let func_path = functions_dir.join(format!("{}.py", func.name));
            if let Ok(node) = tree.get_mut(&func_path) {
                node.set_content(FileContent::from_string(&func.source));
                // Update the span
                node.set_source_fragment(SourceFragment::new(
                    source_path,
                    func.span.start,
                    func.span.end,
                    func.span.start_line,
                    func.span.end_line,
                ));
            }
        }

        info!("Source file reload complete");
    }

    /// Mount the filesystem at the given path.
    pub fn mount(self, mountpoint: &Path) -> Result<(), PythonVfsError> {
        let mut options = vec![
            fuser::MountOption::FSName("codefs".to_string()),
            fuser::MountOption::AutoUnmount,
        ];

        if !self.sync_writes {
            options.push(fuser::MountOption::RO);
        }

        fuser::mount2(self, mountpoint, &options)
            .map_err(|e| PythonVfsError::Mount(e.to_string()))
    }

    /// Mount in the foreground (blocking).
    pub fn mount_foreground(self, mountpoint: &Path) -> Result<(), PythonVfsError> {
        let mut options = vec![fuser::MountOption::FSName("codefs".to_string())];

        if !self.sync_writes {
            options.push(fuser::MountOption::RO);
        }

        fuser::mount2(self, mountpoint, &options)
            .map_err(|e| PythonVfsError::Mount(e.to_string()))
    }

    fn node_to_attr(&self, node: &VfsNode, inode: u64) -> FileAttr {
        let (kind, size, nlink) = match &node.kind {
            NodeKind::File => (FileType::RegularFile, node.size(), 1),
            NodeKind::Directory => (FileType::Directory, 0, 2 + node.children.len() as u32),
            NodeKind::Symlink { target } => {
                (FileType::Symlink, target.len() as u64, 1)
            }
        };

        let mtime = node
            .modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let ctime = node.created.duration_since(UNIX_EPOCH).unwrap_or_default();

        FileAttr {
            ino: inode,
            size,
            blocks: (size + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64,
            atime: SystemTime::now(),
            mtime: UNIX_EPOCH + mtime,
            ctime: UNIX_EPOCH + ctime,
            crtime: UNIX_EPOCH + ctime,
            kind,
            perm: if kind == FileType::Directory {
                0o755
            } else {
                0o644
            },
            nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: BLOCK_SIZE,
            flags: 0,
        }
    }
}

impl Filesystem for PythonFuseFs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        trace!("lookup(parent={}, name={:?})", parent, name);

        let inodes = self.inodes.read().unwrap();
        let Some(parent_path) = inodes.get_path(parent) else {
            reply.error(ENOENT);
            return;
        };

        let child_path = if parent_path.as_os_str() == "/" {
            PathBuf::from("/").join(name)
        } else {
            parent_path.join(name)
        };
        drop(inodes);

        let tree = self.tree.read().unwrap();
        // Convert the path for VFS (strip leading /)
        let vfs_path = child_path
            .strip_prefix("/")
            .unwrap_or(&child_path);

        match tree.get(vfs_path) {
            Ok(node) => {
                let mut inodes = self.inodes.write().unwrap();
                let inode = inodes.get_or_create_inode(&child_path);
                let attr = self.node_to_attr(node, inode);
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => {
                debug!("lookup failed: {:?}", child_path);
                reply.error(ENOENT);
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        trace!("getattr(ino={})", ino);

        let inodes = self.inodes.read().unwrap();
        let Some(path) = inodes.get_path(ino) else {
            reply.error(ENOENT);
            return;
        };
        let path = path.clone();
        drop(inodes);

        let tree = self.tree.read().unwrap();
        let vfs_path = path.strip_prefix("/").unwrap_or(&path);

        match tree.get(vfs_path) {
            Ok(node) => {
                let attr = self.node_to_attr(node, ino);
                reply.attr(&TTL, &attr);
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        trace!("read(ino={}, offset={}, size={})", ino, offset, size);

        let inodes = self.inodes.read().unwrap();
        let Some(path) = inodes.get_path(ino) else {
            reply.error(ENOENT);
            return;
        };
        let path = path.clone();
        drop(inodes);

        let tree = self.tree.read().unwrap();
        let vfs_path = path.strip_prefix("/").unwrap_or(&path);

        match tree.read(vfs_path) {
            Ok(content) => {
                let offset = offset as usize;
                if offset >= content.len() {
                    reply.data(&[]);
                } else {
                    let end = (offset + size as usize).min(content.len());
                    reply.data(&content[offset..end]);
                }
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        trace!("readdir(ino={}, offset={})", ino, offset);

        let inodes_read = self.inodes.read().unwrap();
        let Some(path) = inodes_read.get_path(ino) else {
            reply.error(ENOENT);
            return;
        };
        let path = path.clone();
        drop(inodes_read);

        let tree = self.tree.read().unwrap();
        let vfs_path = path.strip_prefix("/").unwrap_or(&path);

        let node = match tree.get(vfs_path) {
            Ok(node) => node,
            Err(_) => {
                reply.error(ENOENT);
                return;
            }
        };

        if !node.is_dir() {
            reply.error(ENOTDIR);
            return;
        }

        let mut entries = vec![
            (ino, FileType::Directory, ".".to_string()),
            (ino, FileType::Directory, "..".to_string()),
        ];

        let mut inodes = self.inodes.write().unwrap();
        for (name, child) in &node.children {
            let child_path = if path.as_os_str() == "/" {
                PathBuf::from("/").join(name)
            } else {
                path.join(name)
            };
            let child_ino = inodes.get_or_create_inode(&child_path);
            let file_type = match &child.kind {
                NodeKind::File => FileType::RegularFile,
                NodeKind::Directory => FileType::Directory,
                NodeKind::Symlink { .. } => FileType::Symlink,
            };
            entries.push((child_ino, file_type, name.clone()));
        }
        drop(inodes);

        for (i, (inode, file_type, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*inode, (i + 1) as i64, *file_type, name) {
                break;
            }
        }

        reply.ok();
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!(ino = ino, size = ?size, "FUSE setattr operation");

        // Handle truncation if size is specified
        if let Some(new_size) = size {
            info!(
                ino = ino,
                new_size = new_size,
                "FUSE truncate operation started"
            );

            if !self.sync_writes {
                warn!(ino = ino, "Truncate rejected: filesystem mounted read-only");
                reply.error(EROFS);
                return;
            }

            let inodes = self.inodes.read().unwrap();
            let Some(path) = inodes.get_path(ino) else {
                error!(ino = ino, "Truncate failed: inode not found");
                reply.error(ENOENT);
                return;
            };
            let path = path.clone();
            drop(inodes);

            let vfs_path = path.strip_prefix("/").unwrap_or(&path);
            debug!(
                ino = ino,
                vfs_path = %vfs_path.display(),
                new_size = new_size,
                "Truncating file"
            );

            let modified_source = {
                let mut tree = self.tree.write().unwrap();
                match tree.truncate(vfs_path, new_size, self.sync_writes) {
                    Ok(path) => {
                        debug!(
                            vfs_path = %vfs_path.display(),
                            modified_source = ?path,
                            "Truncate completed"
                        );
                        path
                    }
                    Err(e) => {
                        error!(
                            vfs_path = %vfs_path.display(),
                            error = ?e,
                            "Truncate failed"
                        );
                        reply.error(libc::EIO);
                        return;
                    }
                }
            };

            // If a source file was modified, reload it
            if let Some(ref source_path) = modified_source {
                info!(
                    source_file = %source_path.display(),
                    "Source file truncated, triggering reload"
                );
                self.reload_source_file(source_path);
            }

            let tree = self.tree.read().unwrap();
            match tree.get(vfs_path) {
                Ok(node) => {
                    let attr = self.node_to_attr(node, ino);
                    info!(
                        ino = ino,
                        new_size = attr.size,
                        "FUSE truncate operation completed"
                    );
                    reply.attr(&TTL, &attr);
                }
                Err(_) => {
                    error!(ino = ino, "Failed to get node after truncate");
                    reply.error(ENOENT);
                }
            }
        } else {
            // For other setattr operations, just return current attributes
            trace!(ino = ino, "setattr without size change, returning current attrs");
            self.getattr(_req, ino, None, reply);
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        let is_write = (flags & (libc::O_WRONLY | libc::O_RDWR)) != 0;
        let is_truncate = (flags & libc::O_TRUNC) != 0;

        debug!(
            ino = ino,
            flags = flags,
            is_write = is_write,
            is_truncate = is_truncate,
            sync_writes = self.sync_writes,
            "FUSE open operation"
        );

        // Check if write access is requested
        if is_write && !self.sync_writes {
            warn!(
                ino = ino,
                "Open for write rejected: filesystem mounted read-only"
            );
            reply.error(EROFS);
            return;
        }

        // Verify the file exists
        let inodes = self.inodes.read().unwrap();
        let Some(path) = inodes.get_path(ino) else {
            error!(ino = ino, "Open failed: inode not found");
            reply.error(ENOENT);
            return;
        };
        let path = path.clone();
        drop(inodes);

        let tree = self.tree.read().unwrap();
        let vfs_path = path.strip_prefix("/").unwrap_or(&path);

        match tree.get(vfs_path) {
            Ok(node) if node.is_file() => {
                info!(
                    ino = ino,
                    vfs_path = %vfs_path.display(),
                    is_write = is_write,
                    has_source_fragment = node.source_fragment.is_some(),
                    has_source_path = node.source_path.is_some(),
                    "File opened successfully"
                );
                // Use direct I/O to prevent kernel caching issues
                reply.opened(0, fuser::consts::FOPEN_DIRECT_IO);
            }
            Ok(_) => {
                warn!(ino = ino, vfs_path = %vfs_path.display(), "Open failed: is a directory");
                reply.error(libc::EISDIR);
            }
            Err(e) => {
                error!(ino = ino, vfs_path = %vfs_path.display(), error = ?e, "Open failed: not found");
                reply.error(ENOENT);
            }
        }
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        info!(
            ino = ino,
            offset = offset,
            data_len = data.len(),
            "FUSE write operation started"
        );

        if !self.sync_writes {
            warn!(ino = ino, "Write rejected: filesystem mounted read-only");
            reply.error(EROFS);
            return;
        }

        let inodes = self.inodes.read().unwrap();
        let Some(path) = inodes.get_path(ino) else {
            error!(ino = ino, "Write failed: inode not found");
            reply.error(ENOENT);
            return;
        };
        let path = path.clone();
        drop(inodes);

        let vfs_path = path.strip_prefix("/").unwrap_or(&path);
        debug!(
            ino = ino,
            fuse_path = %path.display(),
            vfs_path = %vfs_path.display(),
            "Resolved inode to path"
        );

        // Get current content and build new content
        let new_content = {
            let tree = self.tree.read().unwrap();
            let current_content = match tree.read(vfs_path) {
                Ok(content) => content,
                Err(e) => {
                    error!(
                        vfs_path = %vfs_path.display(),
                        error = ?e,
                        "Failed to read current content"
                    );
                    reply.error(ENOENT);
                    return;
                }
            };

            debug!(
                vfs_path = %vfs_path.display(),
                current_len = current_content.len(),
                write_offset = offset,
                write_len = data.len(),
                "Building new content"
            );

            // Build new content with the write applied
            let offset = offset as usize;
            let mut new_content = current_content;

            // Extend if necessary
            if offset > new_content.len() {
                debug!(
                    current_len = new_content.len(),
                    offset = offset,
                    "Extending content with zeros to reach offset"
                );
                new_content.resize(offset, 0);
            }

            // Apply the write
            let end = offset + data.len();
            if end > new_content.len() {
                debug!(
                    current_len = new_content.len(),
                    end = end,
                    "Extending content to accommodate write"
                );
                new_content.resize(end, 0);
            }
            new_content[offset..end].copy_from_slice(data);

            debug!(
                vfs_path = %vfs_path.display(),
                new_len = new_content.len(),
                "New content built successfully"
            );

            // Log the actual content being written (for debugging)
            if let Ok(content_str) = std::str::from_utf8(data) {
                trace!(
                    vfs_path = %vfs_path.display(),
                    content_preview = %content_str.chars().take(200).collect::<String>(),
                    "Write data preview (first 200 chars)"
                );
            }

            new_content
        };

        // Check if we have a saved source_fragment for this path (from a vim-style backup rename)
        let saved_fragment = {
            let mut saved = self.saved_fragments.write().unwrap();
            saved.remove(&vfs_path.to_path_buf())
        };

        // If we have a saved fragment, restore it to the node before writing
        if let Some(ref fragment) = saved_fragment {
            info!(
                vfs_path = %vfs_path.display(),
                source_file = %fragment.source_path.display(),
                "Restoring saved source_fragment for vim-style save"
            );
            let mut tree = self.tree.write().unwrap();
            if let Ok(node) = tree.get_mut(vfs_path) {
                node.set_source_fragment(fragment.clone());
            }
        }

        info!(
            vfs_path = %vfs_path.display(),
            new_content_len = new_content.len(),
            sync_writes = self.sync_writes,
            has_restored_fragment = saved_fragment.is_some(),
            "Calling VFS tree.write()"
        );

        // Write back and get the modified source file path
        let modified_source = {
            let mut tree = self.tree.write().unwrap();
            match tree.write(vfs_path, new_content, self.sync_writes) {
                Ok(path) => {
                    debug!(
                        vfs_path = %vfs_path.display(),
                        modified_source = ?path,
                        "VFS tree.write() completed"
                    );
                    path
                }
                Err(e) => {
                    error!(
                        vfs_path = %vfs_path.display(),
                        error = ?e,
                        "VFS tree.write() failed"
                    );
                    reply.error(libc::EIO);
                    return;
                }
            }
        };

        // If a source file was modified, defer the reload to release() to avoid corrupting
        // fragment offsets during multi-write operations (e.g., echo or vim writing in chunks)
        if let Some(ref source_path) = modified_source {
            info!(
                source_file = %source_path.display(),
                "Source file modified, deferring reload to release()"
            );
            let mut pending = self.pending_reloads.write().unwrap();
            pending.insert(source_path.clone());
        } else {
            debug!(vfs_path = %vfs_path.display(), "No source file to reload");
        }

        info!(
            ino = ino,
            bytes_written = data.len(),
            "FUSE write operation completed successfully"
        );
        reply.written(data.len() as u32);
    }

    fn flush(&mut self, _req: &Request, ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        trace!("flush(ino={})", ino);
        // Data is already synced in write(), so just acknowledge
        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        debug!(ino = ino, "FUSE release operation");

        // Process any pending source file reloads
        let pending_sources: Vec<PathBuf> = {
            let mut pending = self.pending_reloads.write().unwrap();
            pending.drain().collect()
        };

        if !pending_sources.is_empty() {
            info!(
                count = pending_sources.len(),
                "Processing deferred source file reloads"
            );
            for source_path in pending_sources {
                info!(
                    source_file = %source_path.display(),
                    "Reloading source file (deferred from write)"
                );
                self.reload_source_file(&source_path);
            }
        }

        reply.ok();
    }

    fn fsync(&mut self, _req: &Request, ino: u64, _fh: u64, _datasync: bool, reply: ReplyEmpty) {
        trace!("fsync(ino={})", ino);
        // Data is already synced in write(), so just acknowledge
        reply.ok();
    }

    fn readlink(&mut self, _req: &Request, ino: u64, reply: ReplyData) {
        trace!("readlink(ino={})", ino);

        let inodes = self.inodes.read().unwrap();
        let Some(path) = inodes.get_path(ino) else {
            reply.error(ENOENT);
            return;
        };
        let path = path.clone();
        drop(inodes);

        let tree = self.tree.read().unwrap();
        let vfs_path = path.strip_prefix("/").unwrap_or(&path);

        match tree.get(vfs_path) {
            Ok(node) => {
                if let NodeKind::Symlink { target } = &node.kind {
                    reply.data(target.as_bytes());
                } else {
                    reply.error(libc::EINVAL);
                }
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        trace!("create(parent={}, name={:?}, flags={})", parent, name, flags);

        if !self.sync_writes {
            reply.error(EROFS);
            return;
        }

        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let inodes = self.inodes.read().unwrap();
        let Some(parent_path) = inodes.get_path(parent) else {
            reply.error(ENOENT);
            return;
        };
        let parent_path = parent_path.clone();
        drop(inodes);

        let child_path = if parent_path.as_os_str() == "/" {
            PathBuf::from("/").join(&name_str)
        } else {
            parent_path.join(&name_str)
        };

        let vfs_path = child_path.strip_prefix("/").unwrap_or(&child_path);

        // Create the file in the VFS
        let mut tree = self.tree.write().unwrap();
        if let Err(e) = tree.create_file(vfs_path, FileContent::from_bytes(Vec::new())) {
            warn!("create failed: {:?}", e);
            reply.error(libc::EIO);
            return;
        }

        // Get an inode for the new file
        let mut inodes = self.inodes.write().unwrap();
        let ino = inodes.get_or_create_inode(&child_path);

        match tree.get(vfs_path) {
            Ok(node) => {
                let attr = self.node_to_attr(node, ino);
                reply.created(&TTL, &attr, 0, 0, fuser::consts::FOPEN_DIRECT_IO);
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        trace!("unlink(parent={}, name={:?})", parent, name);

        if !self.sync_writes {
            reply.error(EROFS);
            return;
        }

        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let inodes = self.inodes.read().unwrap();
        let Some(parent_path) = inodes.get_path(parent) else {
            reply.error(ENOENT);
            return;
        };
        let parent_path = parent_path.clone();
        drop(inodes);

        let child_path = if parent_path.as_os_str() == "/" {
            PathBuf::from("/").join(&name_str)
        } else {
            parent_path.join(&name_str)
        };

        let vfs_path = child_path.strip_prefix("/").unwrap_or(&child_path);

        // Check if file has a source path and delete it from disk too
        let mut tree = self.tree.write().unwrap();
        if let Ok(node) = tree.get(vfs_path) {
            if let Some(source_path) = &node.source_path {
                if let Err(e) = std::fs::remove_file(source_path) {
                    warn!("Failed to remove source file {:?}: {}", source_path, e);
                    // Continue anyway - remove from VFS even if source deletion fails
                }
            }
        }

        // Remove from VFS
        if let Err(e) = tree.remove(vfs_path) {
            warn!("unlink failed: {:?}", e);
            reply.error(ENOENT);
            return;
        }

        reply.ok();
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        trace!("mkdir(parent={}, name={:?})", parent, name);

        if !self.sync_writes {
            reply.error(EROFS);
            return;
        }

        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let inodes = self.inodes.read().unwrap();
        let Some(parent_path) = inodes.get_path(parent) else {
            reply.error(ENOENT);
            return;
        };
        let parent_path = parent_path.clone();
        drop(inodes);

        let child_path = if parent_path.as_os_str() == "/" {
            PathBuf::from("/").join(&name_str)
        } else {
            parent_path.join(&name_str)
        };

        let vfs_path = child_path.strip_prefix("/").unwrap_or(&child_path);

        // Create the directory in the VFS
        let mut tree = self.tree.write().unwrap();
        if let Err(e) = tree.mkdir_p(vfs_path) {
            warn!("mkdir failed: {:?}", e);
            reply.error(libc::EIO);
            return;
        }

        // Get an inode for the new directory
        let mut inodes = self.inodes.write().unwrap();
        let ino = inodes.get_or_create_inode(&child_path);

        match tree.get(vfs_path) {
            Ok(node) => {
                let attr = self.node_to_attr(node, ino);
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        info!(
            parent = parent,
            name = ?name,
            newparent = newparent,
            newname = ?newname,
            "FUSE rename operation started"
        );

        if !self.sync_writes {
            warn!("Rename rejected: filesystem mounted read-only");
            reply.error(EROFS);
            return;
        }

        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                error!("Invalid source name encoding");
                reply.error(libc::EINVAL);
                return;
            }
        };

        let newname_str = match newname.to_str() {
            Some(s) => s.to_string(),
            None => {
                error!("Invalid destination name encoding");
                reply.error(libc::EINVAL);
                return;
            }
        };

        let inodes = self.inodes.read().unwrap();
        let Some(parent_path) = inodes.get_path(parent) else {
            error!(parent = parent, "Rename failed: parent inode not found");
            reply.error(ENOENT);
            return;
        };
        let parent_path = parent_path.clone();

        let Some(newparent_path) = inodes.get_path(newparent) else {
            error!(newparent = newparent, "Rename failed: new parent inode not found");
            reply.error(ENOENT);
            return;
        };
        let newparent_path = newparent_path.clone();
        drop(inodes);

        let from_path = if parent_path.as_os_str() == "/" {
            PathBuf::from("/").join(&name_str)
        } else {
            parent_path.join(&name_str)
        };

        let to_path = if newparent_path.as_os_str() == "/" {
            PathBuf::from("/").join(&newname_str)
        } else {
            newparent_path.join(&newname_str)
        };

        let from_vfs = from_path.strip_prefix("/").unwrap_or(&from_path);
        let to_vfs = to_path.strip_prefix("/").unwrap_or(&to_path);

        debug!(
            from_fuse = %from_path.display(),
            to_fuse = %to_path.display(),
            from_vfs = %from_vfs.display(),
            to_vfs = %to_vfs.display(),
            "Resolved rename paths"
        );

        // Before renaming, check if the source file has a source_fragment.
        // If it does, and the destination looks like a backup file (e.g., ends with ~),
        // save the fragment so we can restore it when a new file is created at the original path.
        {
            let tree = self.tree.read().unwrap();
            if let Ok(node) = tree.get(from_vfs) {
                if let Some(ref fragment) = node.source_fragment {
                    // Check if this looks like a backup rename (original -> original~)
                    let to_str = to_vfs.to_string_lossy();
                    let from_str = from_vfs.to_string_lossy();
                    if to_str.starts_with(&*from_str) && to_str.ends_with('~') {
                        info!(
                            from = %from_vfs.display(),
                            to = %to_vfs.display(),
                            source_file = %fragment.source_path.display(),
                            "Saving source_fragment for backup rename (vim-style save)"
                        );
                        let mut saved = self.saved_fragments.write().unwrap();
                        saved.insert(from_vfs.to_path_buf(), fragment.clone());
                    }
                }
            }
        }

        // Perform the rename in VFS
        let modified_source = {
            let mut tree = self.tree.write().unwrap();
            match tree.rename(from_vfs, to_vfs, self.sync_writes) {
                Ok(path) => {
                    debug!(
                        from = %from_vfs.display(),
                        to = %to_vfs.display(),
                        modified_source = ?path,
                        "VFS rename completed"
                    );
                    path
                }
                Err(e) => {
                    error!(
                        from = %from_vfs.display(),
                        to = %to_vfs.display(),
                        error = ?e,
                        "VFS rename failed"
                    );
                    reply.error(libc::EIO);
                    return;
                }
            }
        };

        // Update inode mappings
        let mut inodes = self.inodes.write().unwrap();
        // Remove old path mapping
        if let Some(ino) = inodes.path_to_inode.remove(&from_path) {
            inodes.inode_to_path.remove(&ino);
            // Add new path mapping with the same inode
            inodes.path_to_inode.insert(to_path.clone(), ino);
            inodes.inode_to_path.insert(ino, to_path.clone());
        }
        drop(inodes);

        // If a source file was modified, reload it
        if let Some(ref source_path) = modified_source {
            info!(
                source_file = %source_path.display(),
                "Source file modified via rename, triggering reload"
            );
            self.reload_source_file(source_path);
        }

        info!(
            from = %from_path.display(),
            to = %to_path.display(),
            "FUSE rename operation completed successfully"
        );
        reply.ok();
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        trace!("rmdir(parent={}, name={:?})", parent, name);

        if !self.sync_writes {
            reply.error(EROFS);
            return;
        }

        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let inodes = self.inodes.read().unwrap();
        let Some(parent_path) = inodes.get_path(parent) else {
            reply.error(ENOENT);
            return;
        };
        let parent_path = parent_path.clone();
        drop(inodes);

        let child_path = if parent_path.as_os_str() == "/" {
            PathBuf::from("/").join(&name_str)
        } else {
            parent_path.join(&name_str)
        };

        let vfs_path = child_path.strip_prefix("/").unwrap_or(&child_path);

        let mut tree = self.tree.write().unwrap();

        // Check if directory is empty
        if let Ok(node) = tree.get(vfs_path) {
            if !node.is_dir() {
                reply.error(ENOTDIR);
                return;
            }
            if !node.children.is_empty() {
                reply.error(libc::ENOTEMPTY);
                return;
            }
        } else {
            reply.error(ENOENT);
            return;
        }

        // Remove from VFS
        if let Err(e) = tree.remove(vfs_path) {
            warn!("rmdir failed: {:?}", e);
            reply.error(ENOENT);
            return;
        }

        reply.ok();
    }
}
