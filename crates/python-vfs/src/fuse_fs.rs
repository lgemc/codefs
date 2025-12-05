//! FUSE filesystem implementation.

use crate::error::PythonVfsError;
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request,
    TimeOrNow,
};
use libc::{ENOENT, ENOTDIR};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, trace};
use vfs_core::{NodeKind, VfsNode, VfsTree};

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
}

impl PythonFuseFs {
    /// Create a new Python FUSE filesystem.
    pub fn new(tree: VfsTree) -> Self {
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
        }
    }

    /// Mount the filesystem at the given path.
    pub fn mount(self, mountpoint: &Path) -> Result<(), PythonVfsError> {
        let options = vec![
            fuser::MountOption::RO,
            fuser::MountOption::FSName("codefs".to_string()),
            fuser::MountOption::AutoUnmount,
        ];

        fuser::mount2(self, mountpoint, &options)
            .map_err(|e| PythonVfsError::Mount(e.to_string()))
    }

    /// Mount in the foreground (blocking).
    pub fn mount_foreground(self, mountpoint: &Path) -> Result<(), PythonVfsError> {
        let options = vec![
            fuser::MountOption::RO,
            fuser::MountOption::FSName("codefs".to_string()),
        ];

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
        _size: Option<u64>,
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
        // Read-only filesystem, but we still need to handle setattr for some operations
        trace!("setattr(ino={})", ino);
        self.getattr(_req, ino, None, reply);
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
}
