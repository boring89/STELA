// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A STELA folder as a Mac volume: the host side of the File System
//! Manager plug-in (`c/mac/fsm.c`, `docs/classic.md` §7). Pure logic over
//! a [`Store`] (Classic.app's is `libstela::fs` on `Data:/`; the tests use
//! one in memory).
//!
//! ```text
//!  Mac ID      = the store's id (eHFS: the CNID; the root is 2 on both)
//!  Mac name    = the UTF-8 name in Mac Roman; ':' shows as '/'; a name
//!                with characters Mac Roman lacks ('_' instead) or longer
//!                than 31 ends in "#<ID>"
//!  hidden      = names starting with '.', and the folders `hide` names
//!                at the root (the Classic folder with the running disk)
//!  Finder info = the item's 32 bytes (FInfo + FXInfo, DInfo + DXInfo);
//!                Mouseion's own ("MI") or none: TEXT/ttxt for text files
//!  resource    = the file `.rsrc/<ID>` at the root (the store has no
//!  fork          resource forks)
//! ```

use crate::ffi::{FsItem, FsSpace, MacErr, MacFs, mac_roman_byte, mac_roman_char};
use abi::Error;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Mac OS result codes (Inside Macintosh: Files, "Result Codes";
/// `MAC_FS_..._ERR` in `c/mac/fsm.h`).
pub mod err {
    /// Disk full.
    pub const DSK_FUL: i32 = -34;
    /// I/O error.
    pub const IO: i32 = -36;
    /// Bad name.
    pub const BD_NAM: i32 = -37;
    /// File not found.
    pub const FNF: i32 = -43;
    /// Volume locked.
    pub const WPR: i32 = -44;
    /// File busy (a folder that is not empty).
    pub const FBSY: i32 = -47;
    /// Duplicate file name.
    pub const DUP_FN: i32 = -48;
    /// Folder not found.
    pub const DIR_NF: i32 = -120;
    /// Moving a folder into itself.
    pub const BAD_MOV: i32 = -122;
}

/// The root folder's Mac ID and its parent's.
pub const ROOT_ID: u32 = 2;
/// The root's parent.
pub const ROOT_PARENT: u32 = 1;
/// The folder of the resource forks.
pub const RSRC_DIR: &str = ".rsrc";
/// Seconds from 1904 to 1970.
const MAC_EPOCH: u64 = 2_082_844_800;
/// Largest volume System 7.5 understands.
const VOLUME_MAX: u64 = 2 << 30;

/// One item of a [`Store`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Persistent id.
    pub id: u64,
    /// A folder.
    pub is_dir: bool,
    /// Bytes (files).
    pub size: u64,
    /// Created, ns since 1970.
    pub ctime: u64,
    /// Modified, ns since 1970.
    pub mtime: u64,
    /// The name (UTF-8).
    pub name: String,
}

/// What the volume is made of: folders and files by persistent id.
pub trait Store {
    /// The root folder's id.
    fn root(&self) -> u64;
    /// The item `id`.
    fn get(&mut self, id: u64) -> Result<Node, Error>;
    /// The folder item `id` is in.
    fn parent(&mut self, id: u64) -> Result<u64, Error>;
    /// The items of folder `dir`.
    fn list(&mut self, dir: u64) -> Result<Vec<Node>, Error>;
    /// The item called `name` in `dir` (case is ignored).
    fn lookup(&mut self, dir: u64, name: &str) -> Result<Node, Error>;
    /// Reads at `offset`.
    fn read(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error>;
    /// Writes at `offset`.
    fn write(&mut self, id: u64, offset: u64, data: &[u8]) -> Result<usize, Error>;
    /// Makes a file or folder.
    fn create(&mut self, dir: u64, name: &str, is_dir: bool) -> Result<Node, Error>;
    /// Removes a file or an empty folder.
    fn remove(&mut self, dir: u64, name: &str) -> Result<(), Error>;
    /// Moves and renames.
    fn rename(&mut self, dir: u64, name: &str, new_dir: u64, new_name: &str) -> Result<(), Error>;
    /// Sets a file's size.
    fn set_size(&mut self, id: u64, size: u64) -> Result<(), Error>;
    /// The 32 bytes of Finder information.
    fn finder(&mut self, id: u64) -> Result<[u8; 32], Error>;
    /// Stores the Finder information.
    fn set_finder(&mut self, id: u64, info: &[u8; 32]) -> Result<(), Error>;
    /// (total, free) bytes.
    fn space(&mut self) -> (u64, u64);
}

/// The Mac OS error for a store's error.
pub fn mac_error(e: Error) -> MacErr {
    match e {
        Error::Exists => err::DUP_FN,
        Error::NotFound => err::FNF,
        Error::NotDir => err::DIR_NF,
        Error::NoSpace => err::DSK_FUL,
        Error::NotEmpty => err::FBSY,
        Error::ReadOnly | Error::AccessDenied => err::WPR,
        Error::InvalidArgument => err::BD_NAM,
        _ => err::IO,
    }
}

/// Seconds since 1904 for ns since 1970 (saturating at the 32-bit end).
pub fn mac_seconds(ns: u64) -> u32 {
    (ns / 1_000_000_000 + MAC_EPOCH).min(u64::from(u32::MAX)) as u32
}

/// The Mac's type and creator for a file without Finder information:
/// SimpleText documents for text, nothing in particular otherwise.
pub fn default_type(name: &str) -> ([u8; 4], [u8; 4]) {
    const TEXT: [&str; 12] = [
        "txt", "text", "md", "c", "h", "rs", "py", "lua", "sh", "cfg", "s", "html",
    ];
    match name.rsplit_once('.') {
        Some((_, ext)) if TEXT.iter().any(|t| t.eq_ignore_ascii_case(ext)) => (*b"TEXT", *b"ttxt"),
        _ => (*b"????", *b"????"),
    }
}

/// The Mac name of `name` (item `id`).
pub fn mac_name(name: &str, id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut changed = false;
    for c in name.chars() {
        let b = match c {
            ':' => b'/',
            c if c.is_control() => {
                changed = true;
                b'_'
            }
            c => match mac_roman_byte(c) {
                Some(b) if b >= 0x20 => b,
                _ => {
                    changed = true;
                    b'_'
                }
            },
        };
        out.push(b);
    }
    if changed || out.len() > 31 {
        let suffix = format!("#{id}");
        out.truncate(31 - suffix.len());
        out.extend_from_slice(suffix.as_bytes());
    }
    out
}

/// The STELA name for a Mac name: Mac Roman to UTF-8, '/' stored as ':'.
/// Control characters, ':' and an empty name are refused.
pub fn host_name(mac: &[u8]) -> Result<String, MacErr> {
    if mac.is_empty() || mac.len() > 31 {
        return Err(err::BD_NAM);
    }
    let mut s = String::new();
    for &b in mac {
        match b {
            b'/' => s.push(':'),
            b':' | 0..=0x1F | 0x7F => return Err(err::BD_NAM),
            b => s.push(mac_roman_char(b)),
        }
    }
    Ok(s)
}

/// Mac Roman letters compared without ASCII case.
fn same_name(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// A [`Store`] as a Mac volume.
pub struct HostFs<S: Store> {
    store: S,
    /// The root's name on the Mac (Mac Roman).
    name: Vec<u8>,
    /// Folders at the root the Mac does not see.
    hide: Vec<String>,
    /// The folder each item seen is in.
    parents: BTreeMap<u64, u64>,
    /// The last folder listed (visible items only).
    listing: Option<(u64, Vec<Node>)>,
}

impl<S: Store> HostFs<S> {
    /// The volume `name` over `store`, hiding the root folders in `hide`.
    pub fn new(store: S, name: &str, hide: &[&str]) -> HostFs<S> {
        HostFs {
            store,
            name: mac_name(name, ROOT_ID).into_iter().take(27).collect(),
            hide: hide.iter().map(|h| String::from(*h)).collect(),
            parents: BTreeMap::new(),
            listing: None,
        }
    }

    /// The store (for tests).
    pub fn store(&mut self) -> &mut S {
        &mut self.store
    }

    fn store_id(&self, mac: u32) -> Option<u64> {
        match mac {
            ROOT_ID => Some(self.store.root()),
            0 | ROOT_PARENT => None,
            id => Some(u64::from(id)),
        }
    }

    fn mac_id(&self, id: u64) -> Option<u32> {
        if id == self.store.root() {
            Some(ROOT_ID)
        } else {
            u32::try_from(id).ok().filter(|&i| i > ROOT_ID)
        }
    }

    fn visible(&self, dir: u64, node: &Node) -> bool {
        !node.name.starts_with('.')
            && self.mac_id(node.id).is_some()
            && !(dir == self.store.root()
                && self.hide.iter().any(|h| h.eq_ignore_ascii_case(&node.name)))
    }

    fn children(&mut self, dir: u64) -> Result<Vec<Node>, Error> {
        if let Some((d, items)) = &self.listing
            && *d == dir
        {
            return Ok(items.clone());
        }
        let all = self.store.list(dir)?;
        let items: Vec<Node> = all.into_iter().filter(|n| self.visible(dir, n)).collect();
        for n in &items {
            self.parents.insert(n.id, dir);
        }
        self.listing = Some((dir, items.clone()));
        Ok(items)
    }

    fn changed(&mut self) {
        self.listing = None;
    }

    fn parent_of(&mut self, id: u64) -> Result<u64, Error> {
        if let Some(&p) = self.parents.get(&id) {
            return Ok(p);
        }
        let p = self.store.parent(id)?;
        self.parents.insert(id, p);
        Ok(p)
    }

    fn item(&mut self, node: &Node, parent: u64) -> Option<FsItem> {
        let id = self.mac_id(node.id)?;
        let parent = if node.id == self.store.root() {
            ROOT_PARENT
        } else {
            self.mac_id(parent)?
        };
        let mut finder = self.store.finder(node.id).unwrap_or([0; 32]);
        if &finder[..2] == b"MI" {
            // Mouseion's window state, not the Mac's.
            finder = [0; 32];
        }
        if !node.is_dir && finder[..8] == [0; 8] {
            let (ty, creator) = default_type(&node.name);
            finder[..4].copy_from_slice(&ty);
            finder[4..8].copy_from_slice(&creator);
        }
        let valence = if node.is_dir {
            self.children(node.id).map_or(0, |c| c.len() as u32)
        } else {
            0
        };
        let rsrc_length = if node.is_dir {
            0
        } else {
            self.rsrc(node.id, false)
                .and_then(|r| self.store.get(r).ok())
                .map_or(0, |r| r.size.min(u64::from(u32::MAX)) as u32)
        };
        let name = if id == ROOT_ID {
            self.name.clone()
        } else {
            mac_name(&node.name, id)
        };
        Some(FsItem {
            id,
            parent,
            is_dir: node.is_dir,
            name,
            data_length: node.size.min(u64::from(u32::MAX)) as u32,
            rsrc_length,
            valence,
            finder,
            created: mac_seconds(node.ctime),
            modified: mac_seconds(node.mtime),
        })
    }

    /// The resource fork file of item `id` (made when `make`).
    fn rsrc(&mut self, id: u64, make: bool) -> Option<u64> {
        let root = self.store.root();
        let dir = match self.store.lookup(root, RSRC_DIR) {
            Ok(d) => d.id,
            Err(_) if make => self.store.create(root, RSRC_DIR, true).ok()?.id,
            Err(_) => return None,
        };
        let name = format!("{id}");
        match self.store.lookup(dir, &name) {
            Ok(f) => Some(f.id),
            Err(_) if make => self.store.create(dir, &name, false).ok().map(|f| f.id),
            Err(_) => None,
        }
    }

    fn remove_rsrc(&mut self, id: u64) {
        let root = self.store.root();
        if let Ok(dir) = self.store.lookup(root, RSRC_DIR) {
            let _ = self.store.remove(dir.id, &format!("{id}"));
        }
    }

    /// The fork's file: the item itself or its resource file.
    fn fork(&mut self, mac: u32, rsrc: bool, make: bool) -> Result<Option<u64>, MacErr> {
        let id = self.store_id(mac).ok_or(err::FNF)?;
        if rsrc {
            Ok(self.rsrc(id, make))
        } else {
            Ok(Some(id))
        }
    }
}

impl<S: Store> MacFs for HostFs<S> {
    fn get(&mut self, mac: u32) -> Option<FsItem> {
        let id = self.store_id(mac)?;
        let node = self.store.get(id).ok()?;
        let parent = if id == self.store.root() {
            id
        } else {
            self.parent_of(id).ok()?
        };
        if id != self.store.root() && !self.visible(parent, &node) {
            return None;
        }
        self.item(&node, parent)
    }

    fn lookup(&mut self, parent: u32, name: &[u8]) -> Option<FsItem> {
        let dir = self.store_id(parent)?;
        if let Ok(host) = host_name(name)
            && let Ok(node) = self.store.lookup(dir, &host)
            && self.visible(dir, &node)
        {
            self.parents.insert(node.id, dir);
            return self.item(&node, dir);
        }
        // A name the Mac was given with "#<ID>" at the end.
        let at = name.iter().rposition(|&b| b == b'#')?;
        let id: u64 = core::str::from_utf8(&name[at + 1..]).ok()?.parse().ok()?;
        let node = self.store.get(id).ok()?;
        if self.parent_of(id).ok()? != dir || !self.visible(dir, &node) {
            return None;
        }
        let item = self.item(&node, dir)?;
        same_name(&item.name, name).then_some(item)
    }

    fn child(&mut self, parent: u32, index: u32) -> Option<FsItem> {
        let dir = self.store_id(parent)?;
        let items = self.children(dir).ok()?;
        let node = items
            .get(usize::try_from(index).ok()?.checked_sub(1)?)?
            .clone();
        self.item(&node, dir)
    }

    fn read(&mut self, mac: u32, rsrc: bool, offset: u32, buf: &mut [u8]) -> Option<usize> {
        match self.fork(mac, rsrc, false).ok()? {
            Some(f) => self.store.read(f, u64::from(offset), buf).ok(),
            None => Some(0),
        }
    }

    fn space(&mut self) -> FsSpace {
        let (total, free) = self.store.space();
        FsSpace {
            total: total.min(VOLUME_MAX),
            free: free.min(VOLUME_MAX),
        }
    }

    fn create(&mut self, parent: u32, name: &[u8], is_dir: bool) -> Result<FsItem, MacErr> {
        let dir = self.store_id(parent).ok_or(err::DIR_NF)?;
        let host = host_name(name)?;
        if host.starts_with('.') {
            return Err(err::BD_NAM);
        }
        self.changed();
        let node = self.store.create(dir, &host, is_dir).map_err(mac_error)?;
        self.parents.insert(node.id, dir);
        self.item(&node, dir).ok_or(err::IO)
    }

    fn remove(&mut self, mac: u32) -> Result<(), MacErr> {
        let id = self.store_id(mac).ok_or(err::FNF)?;
        if id == self.store.root() {
            return Err(err::FBSY);
        }
        let node = self.store.get(id).map_err(mac_error)?;
        let dir = self.parent_of(id).map_err(mac_error)?;
        self.changed();
        self.store.remove(dir, &node.name).map_err(mac_error)?;
        self.remove_rsrc(id);
        self.parents.remove(&id);
        Ok(())
    }

    fn move_to(&mut self, mac: u32, parent: u32, name: Option<&[u8]>) -> Result<(), MacErr> {
        let id = self.store_id(mac).ok_or(err::FNF)?;
        let to = self.store_id(parent).ok_or(err::DIR_NF)?;
        if id == self.store.root() {
            return Err(err::BAD_MOV);
        }
        // Not into itself or below.
        let mut at = to;
        while at != self.store.root() {
            if at == id {
                return Err(err::BAD_MOV);
            }
            at = self.parent_of(at).map_err(mac_error)?;
        }
        let node = self.store.get(id).map_err(mac_error)?;
        let from = self.parent_of(id).map_err(mac_error)?;
        let new_name = match name {
            Some(n) => host_name(n)?,
            None => node.name.clone(),
        };
        self.changed();
        self.store
            .rename(from, &node.name, to, &new_name)
            .map_err(mac_error)?;
        self.parents.insert(id, to);
        Ok(())
    }

    fn set_info(&mut self, item: &FsItem) -> Result<(), MacErr> {
        let id = self.store_id(item.id).ok_or(err::FNF)?;
        if id == self.store.root() {
            // The volume's own information is not kept.
            return Ok(());
        }
        self.store.set_finder(id, &item.finder).map_err(mac_error)
    }

    fn write(&mut self, mac: u32, rsrc: bool, offset: u32, data: &[u8]) -> Result<usize, MacErr> {
        let f = self.fork(mac, rsrc, true)?.ok_or(err::IO)?;
        self.changed();
        self.store
            .write(f, u64::from(offset), data)
            .map_err(mac_error)
    }

    fn set_length(&mut self, mac: u32, rsrc: bool, length: u32) -> Result<(), MacErr> {
        self.changed();
        match self.fork(mac, rsrc, length > 0)? {
            Some(f) => self.store.set_size(f, u64::from(length)).map_err(mac_error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    struct Mem {
        parent: u64,
        is_dir: bool,
        name: String,
        data: Vec<u8>,
        finder: [u8; 32],
    }

    /// A store in memory: the root is 2, new ids from 16.
    #[derive(Default)]
    struct MemStore {
        items: BTreeMap<u64, Mem>,
        next: u64,
    }

    impl MemStore {
        fn new() -> MemStore {
            let mut s = MemStore {
                items: BTreeMap::new(),
                next: 16,
            };
            s.items.insert(
                2,
                Mem {
                    parent: 1,
                    is_dir: true,
                    name: String::from("Data"),
                    data: Vec::new(),
                    finder: [0; 32],
                },
            );
            s
        }

        fn node(&self, id: u64) -> Result<Node, Error> {
            let m = self.items.get(&id).ok_or(Error::NotFound)?;
            Ok(Node {
                id,
                is_dir: m.is_dir,
                size: m.data.len() as u64,
                ctime: 1_000_000_000,
                mtime: 2_000_000_000,
                name: m.name.clone(),
            })
        }

        fn find(&self, dir: u64, name: &str) -> Option<u64> {
            self.items
                .iter()
                .find(|(_, m)| m.parent == dir && m.name.eq_ignore_ascii_case(name))
                .map(|(&id, _)| id)
        }
    }

    impl Store for MemStore {
        fn root(&self) -> u64 {
            2
        }
        fn get(&mut self, id: u64) -> Result<Node, Error> {
            self.node(id)
        }
        fn parent(&mut self, id: u64) -> Result<u64, Error> {
            self.items.get(&id).map(|m| m.parent).ok_or(Error::NotFound)
        }
        fn list(&mut self, dir: u64) -> Result<Vec<Node>, Error> {
            let ids: Vec<u64> = self
                .items
                .iter()
                .filter(|(_, m)| m.parent == dir)
                .map(|(&id, _)| id)
                .collect();
            ids.into_iter().map(|id| self.node(id)).collect()
        }
        fn lookup(&mut self, dir: u64, name: &str) -> Result<Node, Error> {
            let id = self.find(dir, name).ok_or(Error::NotFound)?;
            self.node(id)
        }
        fn read(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
            let d = &self.items.get(&id).ok_or(Error::NotFound)?.data;
            let start = (offset as usize).min(d.len());
            let n = buf.len().min(d.len() - start);
            buf[..n].copy_from_slice(&d[start..start + n]);
            Ok(n)
        }
        fn write(&mut self, id: u64, offset: u64, data: &[u8]) -> Result<usize, Error> {
            let d = &mut self.items.get_mut(&id).ok_or(Error::NotFound)?.data;
            let end = offset as usize + data.len();
            if d.len() < end {
                d.resize(end, 0);
            }
            d[offset as usize..end].copy_from_slice(data);
            Ok(data.len())
        }
        fn create(&mut self, dir: u64, name: &str, is_dir: bool) -> Result<Node, Error> {
            if self.find(dir, name).is_some() {
                return Err(Error::Exists);
            }
            let id = self.next;
            self.next += 1;
            self.items.insert(
                id,
                Mem {
                    parent: dir,
                    is_dir,
                    name: String::from(name),
                    data: Vec::new(),
                    finder: [0; 32],
                },
            );
            self.node(id)
        }
        fn remove(&mut self, dir: u64, name: &str) -> Result<(), Error> {
            let id = self.find(dir, name).ok_or(Error::NotFound)?;
            if self.items.values().any(|m| m.parent == id) {
                return Err(Error::NotEmpty);
            }
            self.items.remove(&id);
            Ok(())
        }
        fn rename(&mut self, dir: u64, name: &str, to: u64, new: &str) -> Result<(), Error> {
            let id = self.find(dir, name).ok_or(Error::NotFound)?;
            if self.find(to, new).is_some_and(|other| other != id) {
                return Err(Error::Exists);
            }
            let m = self.items.get_mut(&id).ok_or(Error::NotFound)?;
            m.parent = to;
            m.name = String::from(new);
            Ok(())
        }
        fn set_size(&mut self, id: u64, size: u64) -> Result<(), Error> {
            let m = self.items.get_mut(&id).ok_or(Error::NotFound)?;
            m.data.resize(size as usize, 0);
            Ok(())
        }
        fn finder(&mut self, id: u64) -> Result<[u8; 32], Error> {
            self.items.get(&id).map(|m| m.finder).ok_or(Error::NotFound)
        }
        fn set_finder(&mut self, id: u64, info: &[u8; 32]) -> Result<(), Error> {
            self.items.get_mut(&id).ok_or(Error::NotFound)?.finder = *info;
            Ok(())
        }
        fn space(&mut self) -> (u64, u64) {
            (8 << 30, 3 << 30)
        }
    }

    fn volume() -> HostFs<MemStore> {
        let mut store = MemStore::new();
        let classic = store.create(2, "Classic", true).unwrap().id;
        store.create(classic, "System.img", false).unwrap();
        store.create(2, ".hidden", false).unwrap();
        let notes = store.create(2, "notes.txt", false).unwrap().id;
        store.write(notes, 0, b"hello").unwrap();
        HostFs::new(store, "Data", &["Classic"])
    }

    #[test]
    fn names_both_ways() {
        assert_eq!(mac_name("a:b", 20), b"a/b");
        assert_eq!(host_name(b"a/b").unwrap(), "a:b");
        assert_eq!(mac_name("caf\u{E9}", 20), b"caf\x8E");
        assert_eq!(host_name(b"caf\x8E").unwrap(), "caf\u{E9}");
        assert_eq!(mac_name("\u{6587}.txt", 21), b"_.txt#21");
        let long = "a".repeat(40);
        let m = mac_name(&long, 300);
        assert_eq!(m.len(), 31);
        assert!(m.ends_with(b"#300"));
        assert_eq!(host_name(b"a:b"), Err(err::BD_NAM));
        assert_eq!(host_name(b""), Err(err::BD_NAM));
        assert_eq!(host_name(b"a\x01"), Err(err::BD_NAM));
    }

    #[test]
    fn root_and_listing() {
        let mut v = volume();
        let root = v.get(ROOT_ID).unwrap();
        assert_eq!((root.parent, root.is_dir), (ROOT_PARENT, true));
        assert_eq!(root.name, b"Data");
        // Classic (hidden at the root) and .hidden are not shown.
        assert_eq!(root.valence, 1);
        let first = v.child(ROOT_ID, 1).unwrap();
        assert_eq!(first.name, b"notes.txt");
        assert_eq!(first.data_length, 5);
        assert_eq!(&first.finder[..8], b"TEXTttxt");
        assert_eq!(first.modified, 2 + 2_082_844_800);
        assert!(v.child(ROOT_ID, 2).is_none());
        assert!(v.child(ROOT_ID, 0).is_none());
        assert!(v.lookup(ROOT_ID, b"Classic").is_none());
        assert!(v.lookup(ROOT_ID, b".hidden").is_none());
        assert_eq!(v.lookup(ROOT_ID, b"NOTES.TXT").unwrap().id, first.id);
        let mut buf = [0u8; 8];
        assert_eq!(v.read(first.id, false, 1, &mut buf), Some(4));
        assert_eq!(&buf[..4], b"ello");
        assert_eq!(v.read(first.id, true, 0, &mut buf), Some(0));
        let space = v.space();
        assert_eq!((space.total, space.free), (2 << 30, 2 << 30));
    }

    #[test]
    fn suffixed_names_are_found() {
        let mut v = volume();
        let id = v.store().create(2, "\u{6587}", true).unwrap().id as u32;
        let name = mac_name("\u{6587}", id);
        let item = v.lookup(ROOT_ID, &name).unwrap();
        assert_eq!(item.id, id);
        assert!(item.is_dir);
        assert!(v.lookup(ROOT_ID, b"_#9999").is_none());
    }

    #[test]
    fn writing() {
        let mut v = volume();
        let folder = v.create(ROOT_ID, b"Stuff", true).unwrap();
        assert!(folder.is_dir);
        assert_eq!(v.create(ROOT_ID, b"stuff", false), Err(err::DUP_FN));
        assert_eq!(v.create(ROOT_ID, b".x", false), Err(err::BD_NAM));
        let file = v.create(folder.id, b"Doc", false).unwrap();
        assert_eq!(file.parent, folder.id);
        assert_eq!(v.write(file.id, false, 0, b"abc"), Ok(3));
        assert_eq!(v.write(file.id, true, 0, b"rsrc!"), Ok(5));
        let again = v.get(file.id).unwrap();
        assert_eq!((again.data_length, again.rsrc_length), (3, 5));
        let mut buf = [0u8; 5];
        assert_eq!(v.read(file.id, true, 0, &mut buf), Some(5));
        assert_eq!(&buf, b"rsrc!");
        assert_eq!(v.set_length(file.id, false, 1), Ok(()));
        assert_eq!(v.get(file.id).unwrap().data_length, 1);
        // Finder information is kept as given.
        let mut info = again.clone();
        info.finder[..8].copy_from_slice(b"APPLttxt");
        info.finder[10] = 7;
        assert_eq!(v.set_info(&info), Ok(()));
        assert_eq!(&v.get(file.id).unwrap().finder[..11], b"APPLttxt\0\0\x07");
        // Moving and renaming keep the ID; a folder cannot go into itself.
        assert_eq!(v.move_to(file.id, ROOT_ID, Some(b"Doc 2")), Ok(()));
        let moved = v.get(file.id).unwrap();
        assert_eq!(
            (moved.parent, moved.name.as_slice()),
            (ROOT_ID, &b"Doc 2"[..])
        );
        let inner = v.create(folder.id, b"Inner", true).unwrap();
        assert_eq!(v.move_to(folder.id, inner.id, None), Err(err::BAD_MOV));
        assert_eq!(v.remove(folder.id), Err(err::FBSY));
        assert_eq!(v.remove(inner.id), Ok(()));
        assert_eq!(v.remove(folder.id), Ok(()));
        assert_eq!(v.remove(file.id), Ok(()));
        assert!(v.get(file.id).is_none());
        // Its resource fork went with it.
        let root = v.get(ROOT_ID).unwrap();
        assert_eq!(root.valence, 1);
        assert!(v.store().lookup(2, RSRC_DIR).is_ok());
        let rsrc_dir = v.store().lookup(2, RSRC_DIR).unwrap().id;
        assert_eq!(v.store().list(rsrc_dir).unwrap(), vec![]);
    }

    #[test]
    fn mouseion_info_is_not_the_macs() {
        let mut v = volume();
        let notes = v.lookup(ROOT_ID, b"notes.txt").unwrap().id;
        let mut mi = [0u8; 32];
        mi[..4].copy_from_slice(b"MI\x01\x02");
        v.store().set_finder(u64::from(notes), &mi).unwrap();
        assert_eq!(&v.get(notes).unwrap().finder[..8], b"TEXTttxt");
        assert_eq!(default_type("a.BIN"), (*b"????", *b"????"));
    }
}
