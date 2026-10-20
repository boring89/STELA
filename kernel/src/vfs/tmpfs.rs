// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `Tmp`: a file system in kernel memory (Linux programs see it as
//! `/tmp`). Everything is lost at shutdown. Pure logic, tested on the
//! host; `target.rs` wraps it as a VFS backend.
//!
//! ```text
//!  id 1  /            nodes: BTreeMap<id, Node>
//!  id 2  /notes.txt   a file is a Vec<u8>; all files together ≤ 16 MiB
//!  id 3  /d/          names: case kept, compared ignoring ASCII case
//! ```

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Most bytes all files may hold together.
pub const CAPACITY: u64 = 16 << 20;
/// The root folder's id.
pub const ROOT: u64 = 1;
/// Longest name.
pub const NAME_MAX: usize = 255;

/// Why an operation failed (the VFS maps these to `abi::Error`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TmpError {
    /// No such item.
    NotFound,
    /// The name is taken.
    Exists,
    /// Not a folder.
    NotDir,
    /// A folder where a file is needed.
    IsDir,
    /// The folder is not empty.
    NotEmpty,
    /// Over [`CAPACITY`].
    NoSpace,
    /// An empty, too long or `/`-containing name, or moving a folder into
    /// itself.
    Invalid,
}

/// A file or folder.
#[derive(Clone, Debug)]
pub struct Node {
    /// The folder it is in (the root: itself).
    pub parent: u64,
    /// Its name.
    pub name: String,
    /// `Some` for files.
    pub data: Option<Vec<u8>>,
    /// Creation time (ns since 1970).
    pub ctime: u64,
    /// Last change.
    pub mtime: u64,
}

impl Node {
    /// A folder.
    pub fn is_dir(&self) -> bool {
        self.data.is_none()
    }

    /// Bytes (0 for folders).
    pub fn size(&self) -> u64 {
        self.data.as_ref().map_or(0, |d| d.len() as u64)
    }
}

/// The whole file system.
#[derive(Debug)]
pub struct TmpTree {
    nodes: BTreeMap<u64, Node>,
    next: u64,
    used: u64,
}

fn check_name(name: &str) -> Result<(), TmpError> {
    if name.is_empty() || name.len() > NAME_MAX || name.contains('/') || name == "." || name == ".."
    {
        return Err(TmpError::Invalid);
    }
    Ok(())
}

impl Default for TmpTree {
    fn default() -> Self {
        Self::new()
    }
}

impl TmpTree {
    /// An empty tree.
    pub fn new() -> TmpTree {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            ROOT,
            Node {
                parent: ROOT,
                name: String::new(),
                data: None,
                ctime: 0,
                mtime: 0,
            },
        );
        TmpTree {
            nodes,
            next: ROOT + 1,
            used: 0,
        }
    }

    /// Bytes in use.
    pub fn used(&self) -> u64 {
        self.used
    }

    /// The node `id`.
    pub fn get(&self, id: u64) -> Result<&Node, TmpError> {
        self.nodes.get(&id).ok_or(TmpError::NotFound)
    }

    fn folder(&self, dir: u64) -> Result<&Node, TmpError> {
        let n = self.get(dir)?;
        if !n.is_dir() {
            return Err(TmpError::NotDir);
        }
        Ok(n)
    }

    /// The items of folder `dir`: (id, node), in id order.
    pub fn list(&self, dir: u64) -> Result<Vec<(u64, &Node)>, TmpError> {
        self.folder(dir)?;
        Ok(self
            .nodes
            .iter()
            .filter(|&(&id, n)| n.parent == dir && id != ROOT)
            .map(|(&id, n)| (id, n))
            .collect())
    }

    /// The item `name` in `dir`.
    pub fn lookup(&self, dir: u64, name: &str) -> Result<u64, TmpError> {
        self.list(dir)?
            .into_iter()
            .find(|(_, n)| n.name.eq_ignore_ascii_case(name))
            .map(|(id, _)| id)
            .ok_or(TmpError::NotFound)
    }

    /// Makes a file or folder `name` in `dir`.
    pub fn create(
        &mut self,
        dir: u64,
        name: &str,
        is_dir: bool,
        now: u64,
    ) -> Result<u64, TmpError> {
        check_name(name)?;
        self.folder(dir)?;
        if self.lookup(dir, name).is_ok() {
            return Err(TmpError::Exists);
        }
        let id = self.next;
        self.next += 1;
        self.nodes.insert(
            id,
            Node {
                parent: dir,
                name: name.into(),
                data: (!is_dir).then(Vec::new),
                ctime: now,
                mtime: now,
            },
        );
        if let Some(parent) = self.nodes.get_mut(&dir) {
            parent.mtime = now;
        }
        Ok(id)
    }

    /// Removes the file or empty folder `name` from `dir`.
    pub fn remove(&mut self, dir: u64, name: &str) -> Result<(), TmpError> {
        let id = self.lookup(dir, name)?;
        if self.get(id)?.is_dir() && !self.list(id)?.is_empty() {
            return Err(TmpError::NotEmpty);
        }
        if let Some(n) = self.nodes.remove(&id) {
            self.used -= n.size();
        }
        Ok(())
    }

    /// Moves `dir/name` to `new_dir/new_name` (which must be free).
    pub fn rename(
        &mut self,
        dir: u64,
        name: &str,
        new_dir: u64,
        new_name: &str,
    ) -> Result<(), TmpError> {
        check_name(new_name)?;
        let id = self.lookup(dir, name)?;
        self.folder(new_dir)?;
        if let Ok(other) = self.lookup(new_dir, new_name)
            && other != id
        {
            return Err(TmpError::Exists);
        }
        // A folder cannot go inside itself.
        let mut at = new_dir;
        while at != ROOT {
            if at == id {
                return Err(TmpError::Invalid);
            }
            at = self.get(at)?.parent;
        }
        let n = self.nodes.get_mut(&id).ok_or(TmpError::NotFound)?;
        n.parent = new_dir;
        n.name = new_name.into();
        Ok(())
    }

    /// Reads at `offset` of file `id`.
    pub fn read(&self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, TmpError> {
        let data = self.get(id)?.data.as_ref().ok_or(TmpError::IsDir)?;
        let Some(rest) = usize::try_from(offset).ok().and_then(|o| data.get(o..)) else {
            return Ok(0);
        };
        let n = rest.len().min(buf.len());
        buf[..n].copy_from_slice(&rest[..n]);
        Ok(n)
    }

    /// Changes file `id`'s size, keeping the capacity.
    pub fn set_size(&mut self, id: u64, size: u64, now: u64) -> Result<(), TmpError> {
        let used = self.used;
        let n = self.nodes.get_mut(&id).ok_or(TmpError::NotFound)?;
        let data = n.data.as_mut().ok_or(TmpError::IsDir)?;
        let new_used = used - data.len() as u64 + size;
        if size > data.len() as u64 && new_used > CAPACITY {
            return Err(TmpError::NoSpace);
        }
        data.resize(size as usize, 0);
        n.mtime = now;
        self.used = new_used;
        Ok(())
    }

    /// Writes `bytes` at `offset` of file `id` (growing it as needed).
    pub fn write(&mut self, id: u64, offset: u64, bytes: &[u8], now: u64) -> Result<(), TmpError> {
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(TmpError::NoSpace)?;
        let size = self.get(id)?.size();
        if end > size {
            self.set_size(id, end, now)?;
        }
        let n = self.nodes.get_mut(&id).ok_or(TmpError::NotFound)?;
        let data = n.data.as_mut().ok_or(TmpError::IsDir)?;
        data[offset as usize..end as usize].copy_from_slice(bytes);
        n.mtime = now;
        Ok(())
    }
}

#[cfg(target_os = "none")]
pub use target::TmpFs;

#[cfg(target_os = "none")]
mod target {
    use super::{TmpError, TmpTree};
    use crate::vfs::{FileSystem, NodeInfo};
    use abi::Error;
    use alloc::vec::Vec;
    use frame::sync::SpinLock;

    fn error(e: TmpError) -> Error {
        match e {
            TmpError::NotFound => Error::NotFound,
            TmpError::Exists => Error::Exists,
            TmpError::NotDir => Error::NotDir,
            TmpError::IsDir => Error::IsDir,
            TmpError::NotEmpty => Error::NotEmpty,
            TmpError::NoSpace => Error::NoSpace,
            TmpError::Invalid => Error::InvalidArgument,
        }
    }

    /// The `Tmp` volume's backend.
    pub struct TmpFs {
        tree: SpinLock<TmpTree>,
    }

    impl Default for TmpFs {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TmpFs {
        /// An empty memory file system.
        pub fn new() -> TmpFs {
            TmpFs {
                tree: SpinLock::new(TmpTree::new()),
            }
        }
    }

    fn info(tree: &TmpTree, id: u64) -> Result<NodeInfo, Error> {
        let n = tree.get(id).map_err(error)?;
        Ok(NodeInfo {
            id,
            is_dir: n.is_dir(),
            size: n.size(),
            ctime: n.ctime,
            mtime: n.mtime,
            name: n.name.clone(),
        })
    }

    impl FileSystem for TmpFs {
        fn root(&self) -> u64 {
            super::ROOT
        }

        fn lookup(&self, dir: u64, name: &str) -> Result<NodeInfo, Error> {
            let tree = self.tree.lock();
            let id = tree.lookup(dir, name).map_err(error)?;
            info(&tree, id)
        }

        fn get(&self, id: u64) -> Result<NodeInfo, Error> {
            info(&self.tree.lock(), id)
        }

        fn list(&self, dir: u64) -> Result<Vec<NodeInfo>, Error> {
            let tree = self.tree.lock();
            let ids: Vec<u64> = tree
                .list(dir)
                .map_err(error)?
                .iter()
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter().map(|id| info(&tree, id)).collect()
        }

        fn read(&self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
            self.tree.lock().read(id, offset, buf).map_err(error)
        }

        fn write(&self, id: u64, offset: u64, data: &[u8]) -> Result<(), Error> {
            let now = crate::fs::now();
            self.tree.lock().write(id, offset, data, now).map_err(error)
        }

        fn create(&self, dir: u64, name: &str, is_dir: bool) -> Result<NodeInfo, Error> {
            let now = crate::fs::now();
            let mut tree = self.tree.lock();
            let id = tree.create(dir, name, is_dir, now).map_err(error)?;
            info(&tree, id)
        }

        fn remove(&self, dir: u64, name: &str) -> Result<(), Error> {
            self.tree.lock().remove(dir, name).map_err(error)
        }

        fn rename(&self, dir: u64, name: &str, new_dir: u64, new_name: &str) -> Result<(), Error> {
            self.tree
                .lock()
                .rename(dir, name, new_dir, new_name)
                .map_err(error)
        }

        fn set_size(&self, id: u64, size: u64) -> Result<(), Error> {
            let now = crate::fs::now();
            self.tree.lock().set_size(id, size, now).map_err(error)
        }

        fn read_only(&self) -> bool {
            false
        }

        fn space(&self) -> (u64, u64) {
            let used = self.tree.lock().used();
            (super::CAPACITY, super::CAPACITY - used)
        }

        fn kind(&self) -> u32 {
            abi::volume_kind::TMP
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_folders() {
        let mut t = TmpTree::new();
        let d = t.create(ROOT, "Dir", true, 5).unwrap();
        let f = t.create(d, "a.txt", false, 6).unwrap();
        assert_eq!(t.lookup(ROOT, "dir"), Ok(d));
        assert_eq!(t.create(ROOT, "DIR", false, 7), Err(TmpError::Exists));
        t.write(f, 0, b"hello", 8).unwrap();
        t.write(f, 10, b"!", 9).unwrap();
        assert_eq!(t.get(f).unwrap().size(), 11);
        let mut buf = [0u8; 16];
        assert_eq!(t.read(f, 0, &mut buf), Ok(11));
        assert_eq!(&buf[..11], b"hello\0\0\0\0\0!");
        assert_eq!(t.read(f, 100, &mut buf), Ok(0));
        assert_eq!(t.used(), 11);
        assert_eq!(t.read(d, 0, &mut buf), Err(TmpError::IsDir));
        assert_eq!(t.remove(ROOT, "dir"), Err(TmpError::NotEmpty));
        t.rename(d, "a.txt", ROOT, "b.txt").unwrap();
        assert_eq!(t.lookup(ROOT, "b.txt"), Ok(f));
        assert_eq!(t.list(d).unwrap().len(), 0);
        t.remove(ROOT, "dir").unwrap();
        t.remove(ROOT, "b.txt").unwrap();
        assert_eq!(t.used(), 0);
        assert_eq!(t.list(ROOT).unwrap().len(), 0);
    }

    #[test]
    fn limits_and_errors() {
        let mut t = TmpTree::new();
        let f = t.create(ROOT, "big", false, 0).unwrap();
        assert_eq!(t.set_size(f, CAPACITY + 1, 0), Err(TmpError::NoSpace));
        t.set_size(f, CAPACITY, 0).unwrap();
        let g = t.create(ROOT, "g", false, 0).unwrap();
        assert_eq!(t.write(g, 0, b"x", 0), Err(TmpError::NoSpace));
        t.set_size(f, 0, 0).unwrap();
        t.write(g, 0, b"x", 0).unwrap();
        assert_eq!(t.create(ROOT, "", false, 0), Err(TmpError::Invalid));
        assert_eq!(t.create(ROOT, "a/b", false, 0), Err(TmpError::Invalid));
        assert_eq!(t.create(f, "x", false, 0), Err(TmpError::NotDir));
        assert_eq!(t.lookup(ROOT, "nothing"), Err(TmpError::NotFound));
        let a = t.create(ROOT, "a", true, 0).unwrap();
        let b = t.create(a, "b", true, 0).unwrap();
        assert_eq!(t.rename(ROOT, "a", b, "a"), Err(TmpError::Invalid));
        assert_eq!(t.rename(ROOT, "g", ROOT, "big"), Err(TmpError::Exists));
    }
}
