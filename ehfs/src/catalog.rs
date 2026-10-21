// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The catalog (68k_HAL `docs/ehfs.md` sections 4.2 and 11.2): folder and
//! file records keyed by (parent CNID big endian, comparison form of the
//! name), and a thread record per item keyed by its own CNID (8 bytes, so
//! a folder's thread sorts just before its items) that leads back to
//! (parent, name). The functions that change it run inside a transaction.

use crate::Error;
use crate::format::*;
use crate::name;
use crate::volume::Volume;
use alloc::vec;
use alloc::vec::Vec;
use block::BlockDevice;

/// A file or folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its CNID.
    pub cnid: u64,
    /// The folder it is in.
    pub parent: u64,
    /// A folder.
    pub is_dir: bool,
    /// Bit 0: locked.
    pub flags: u16,
    /// Folders: items inside.
    pub valence: u64,
    /// Created, modified, attributes changed, accessed, backed up (ns
    /// since 1970, UTC).
    pub times: [u64; TIME_COUNT],
    /// FInfo + FXInfo (files) or DInfo + DXInfo (folders).
    pub finder: [u8; 32],
    /// Files: the data fork descriptor.
    pub data_fork: [u8; FORK_SIZE],
    /// Files: the resource fork descriptor.
    pub rsrc_fork: [u8; FORK_SIZE],
    /// The name as it was given (UTF-8).
    pub name: Vec<u8>,
}

impl Entry {
    fn empty() -> Entry {
        Entry {
            cnid: 0,
            parent: 0,
            is_dir: false,
            flags: 0,
            valence: 0,
            times: [0; TIME_COUNT],
            finder: [0; 32],
            data_fork: [0; FORK_SIZE],
            rsrc_fork: [0; FORK_SIZE],
            name: Vec::new(),
        }
    }

    /// The length of the data fork in bytes.
    pub fn size(&self) -> u64 {
        le64(&self.data_fork, fork::LENGTH)
    }

    /// The fork descriptor (`resource` = the resource fork).
    pub fn fork(&self, resource: bool) -> &[u8; FORK_SIZE] {
        if resource {
            &self.rsrc_fork
        } else {
            &self.data_fork
        }
    }

    pub(crate) fn fork_mut(&mut self, resource: bool) -> &mut [u8; FORK_SIZE] {
        if resource {
            &mut self.rsrc_fork
        } else {
            &mut self.data_fork
        }
    }
}

/// A B-tree record copied out: (key, value).
pub(crate) type Record = (Vec<u8>, Vec<u8>);

pub(crate) fn be(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

/// (parent, comparison form of `name`).
pub(crate) fn make_key(parent: u64, name: &[u8]) -> Result<Vec<u8>, Error> {
    let form = name::key(name).ok_or(Error::BadName)?;
    let mut k = be(parent).to_vec();
    k.extend_from_slice(&form);
    Ok(k)
}

fn encode(e: &Entry) -> Vec<u8> {
    let name_at = if e.is_dir {
        rec::FOLDER_NAME
    } else {
        rec::FILE_NAME
    };
    let mut r = vec![0u8; name_at + 2 + e.name.len()];
    put16(
        &mut r,
        rec::TYPE,
        if e.is_dir { rec::FOLDER } else { rec::FILE },
    );
    put16(&mut r, rec::FLAGS, e.flags);
    put64(&mut r, rec::CNID, e.cnid);
    put64(&mut r, rec::VALENCE, if e.is_dir { e.valence } else { 0 });
    for (i, t) in e.times.iter().enumerate() {
        put64(&mut r, rec::TIMES + 8 * i, *t);
    }
    r[rec::FINDER..rec::FINDER + 32].copy_from_slice(&e.finder);
    if !e.is_dir {
        r[rec::DATA_FORK..rec::DATA_FORK + FORK_SIZE].copy_from_slice(&e.data_fork);
        r[rec::RSRC_FORK..rec::RSRC_FORK + FORK_SIZE].copy_from_slice(&e.rsrc_fork);
    }
    put16(&mut r, name_at, e.name.len() as u16);
    r[name_at + 2..].copy_from_slice(&e.name);
    r
}

pub(crate) fn decode(r: &[u8], parent: u64) -> Result<Entry, Error> {
    if r.len() < 4 {
        return Err(Error::Corrupt);
    }
    let kind = le16(r, rec::TYPE);
    if kind != rec::FOLDER && kind != rec::FILE {
        return Err(Error::Corrupt);
    }
    let name_at = if kind == rec::FOLDER {
        rec::FOLDER_NAME
    } else {
        rec::FILE_NAME
    };
    if r.len() < name_at + 2 {
        return Err(Error::Corrupt);
    }
    let n = usize::from(le16(r, name_at));
    if n > name::NAME_MAX || r.len() != name_at + 2 + n {
        return Err(Error::Corrupt);
    }
    let mut e = Entry::empty();
    e.is_dir = kind == rec::FOLDER;
    e.parent = parent;
    e.flags = le16(r, rec::FLAGS);
    e.cnid = le64(r, rec::CNID);
    e.valence = le64(r, rec::VALENCE);
    for i in 0..TIME_COUNT {
        e.times[i] = le64(r, rec::TIMES + 8 * i);
    }
    e.finder.copy_from_slice(&r[rec::FINDER..rec::FINDER + 32]);
    if !e.is_dir {
        e.data_fork
            .copy_from_slice(&r[rec::DATA_FORK..rec::DATA_FORK + FORK_SIZE]);
        e.rsrc_fork
            .copy_from_slice(&r[rec::RSRC_FORK..rec::RSRC_FORK + FORK_SIZE]);
    }
    e.name = r[name_at + 2..].to_vec();
    Ok(e)
}

fn encode_thread(parent: u64, is_dir: bool, name: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8; rec::THREAD_NAME + 2 + name.len()];
    put16(
        &mut r,
        rec::TYPE,
        if is_dir {
            rec::FOLDER_THREAD
        } else {
            rec::FILE_THREAD
        },
    );
    put64(&mut r, rec::THREAD_PARENT, parent);
    put16(&mut r, rec::THREAD_NAME, name.len() as u16);
    r[rec::THREAD_NAME + 2..].copy_from_slice(name);
    r
}

/// (parent, name, is_dir) of a thread record.
pub(crate) fn decode_thread(r: &[u8]) -> Result<(u64, Vec<u8>, bool), Error> {
    if r.len() < rec::THREAD_NAME + 2
        || r.len() != rec::THREAD_NAME + 2 + usize::from(le16(r, rec::THREAD_NAME))
    {
        return Err(Error::Corrupt);
    }
    let kind = le16(r, rec::TYPE);
    if kind != rec::FOLDER_THREAD && kind != rec::FILE_THREAD {
        return Err(Error::Corrupt);
    }
    Ok((
        le64(r, rec::THREAD_PARENT),
        r[rec::THREAD_NAME + 2..].to_vec(),
        kind == rec::FOLDER_THREAD,
    ))
}

impl<D: BlockDevice> Volume<D> {
    /// The item called `name` in folder `parent`.
    pub fn lookup(&mut self, parent: u64, name: &[u8]) -> Result<Entry, Error> {
        let key = make_key(parent, name)?;
        let r = self.bt_find(Special::Catalog, &key)?;
        decode(&r, parent)
    }

    /// The item with this CNID (through its thread).
    pub fn get(&mut self, cnid: u64) -> Result<Entry, Error> {
        let r = self.bt_find(Special::Catalog, &be(cnid))?;
        let (parent, name, _) = decode_thread(&r)?;
        match self.lookup(parent, &name) {
            Ok(e) if e.cnid == cnid => Ok(e),
            Ok(_) | Err(Error::NotFound) => Err(Error::Corrupt),
            Err(e) => Err(e),
        }
    }

    /// The items of folder `parent`, in the order of their comparison
    /// forms.
    pub fn list(&mut self, parent: u64) -> Result<Vec<Entry>, Error> {
        let mut out = Vec::new();
        let start = be(parent);
        let mut bad = false;
        self.bt_walk(Special::Catalog, &start, |k, v| {
            if k.len() < 8 || be64(k) != parent {
                return true;
            }
            if k.len() == 8 {
                return false; // the folder's own thread
            }
            match decode(v, parent) {
                Ok(e) => out.push(e),
                Err(_) => bad = true,
            }
            bad
        })?;
        if bad {
            return Err(Error::Corrupt);
        }
        Ok(out)
    }

    /// Up to `max` items of folder `parent` that come after the name
    /// `after` in key order (from the first when `None`): a B-tree walk
    /// from the lower bound of (parent, after), so a long listing costs
    /// O(n) in all instead of O(n) per item (P11). `after` need not exist
    /// any more.
    pub fn list_after(
        &mut self,
        parent: u64,
        after: Option<&[u8]>,
        max: usize,
    ) -> Result<Vec<Entry>, Error> {
        let start = match after {
            Some(name) => make_key(parent, name)?,
            None => be(parent).to_vec(),
        };
        let mut out = Vec::new();
        let mut bad = false;
        self.bt_walk(Special::Catalog, &start, |k, v| {
            if k.len() < 8 || be64(k) != parent || out.len() >= max {
                return true;
            }
            if k.len() == 8 || (after.is_some() && k == start.as_slice()) {
                return false; // the folder's own thread, or `after` itself
            }
            match decode(v, parent) {
                Ok(e) => out.push(e),
                Err(_) => bad = true,
            }
            bad
        })?;
        if bad {
            return Err(Error::Corrupt);
        }
        Ok(out)
    }

    fn adjust_folder(&mut self, cnid: u64, delta: i64, now: u64) -> Result<(), Error> {
        let mut folder = self.get(cnid)?;
        if !folder.is_dir {
            return Err(Error::NotDir);
        }
        folder.valence = (folder.valence as i64 + delta) as u64;
        folder.times[1] = now;
        self.cat_update(&folder)
    }

    /// Writes an item's record back (its place and name stay).
    pub(crate) fn cat_update(&mut self, e: &Entry) -> Result<(), Error> {
        let key = make_key(e.parent, &e.name)?;
        self.bt_replace(Special::Catalog, &key, &encode(e))
    }

    fn insert_item(&mut self, e: &Entry) -> Result<(), Error> {
        let key = make_key(e.parent, &e.name)?;
        self.bt_insert(Special::Catalog, &key, &encode(e))?;
        self.bt_insert(
            Special::Catalog,
            &be(e.cnid),
            &encode_thread(e.parent, e.is_dir, &e.name),
        )
    }

    fn delete_item(&mut self, e: &Entry) -> Result<(), Error> {
        let key = make_key(e.parent, &e.name)?;
        self.bt_delete(Special::Catalog, &key)?;
        self.bt_delete(Special::Catalog, &be(e.cnid))
    }

    /// Makes a new empty file or folder (in the open transaction).
    pub(crate) fn cat_create(
        &mut self,
        parent: u64,
        name: &[u8],
        is_dir: bool,
        now: u64,
    ) -> Result<Entry, Error> {
        if !name::valid(name) {
            return Err(Error::BadName);
        }
        let folder = self.get(parent)?;
        if !folder.is_dir {
            return Err(Error::NotDir);
        }
        match self.lookup(parent, name) {
            Ok(_) => return Err(Error::Exists),
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        let cnid = self.sb_get64(sb::NEXT_CNID);
        if cnid > cnid::MAC_MAX {
            return Err(Error::NoSpace);
        }
        let mut e = Entry::empty();
        e.cnid = cnid;
        e.parent = parent;
        e.is_dir = is_dir;
        e.times[..4].fill(now);
        e.name = name.to_vec();
        self.insert_item(&e)?;
        self.adjust_folder(parent, 1, now)?;
        self.sb_set64(sb::NEXT_CNID, cnid + 1);
        let at = if is_dir {
            sb::FOLDER_COUNT
        } else {
            sb::FILE_COUNT
        };
        let n = self.sb_get64(at);
        self.sb_set64(at, n + 1);
        Ok(e)
    }

    /// Removes the record and thread of an item (a folder must be empty).
    pub(crate) fn cat_remove(&mut self, parent: u64, name: &[u8], now: u64) -> Result<(), Error> {
        let e = self.lookup(parent, name)?;
        if e.cnid == cnid::ROOT {
            return Err(Error::Invalid);
        }
        if e.is_dir && e.valence != 0 {
            return Err(Error::NotEmpty);
        }
        self.delete_item(&e)?;
        self.adjust_folder(parent, -1, now)?;
        let at = if e.is_dir {
            sb::FOLDER_COUNT
        } else {
            sb::FILE_COUNT
        };
        let n = self.sb_get64(at);
        self.sb_set64(at, n - 1);
        Ok(())
    }

    /// Moves and/or renames an item.
    pub(crate) fn cat_move(
        &mut self,
        parent: u64,
        name: &[u8],
        new_parent: u64,
        new_name: &[u8],
        now: u64,
    ) -> Result<(), Error> {
        if !name::valid(new_name) {
            return Err(Error::BadName);
        }
        let mut e = self.lookup(parent, name)?;
        if e.cnid == cnid::ROOT {
            return Err(Error::Invalid);
        }
        let folder = self.get(new_parent)?;
        if !folder.is_dir {
            return Err(Error::NotDir);
        }
        match self.lookup(new_parent, new_name) {
            Ok(other) if other.cnid != e.cnid => return Err(Error::Exists),
            Ok(_) | Err(Error::NotFound) => {}
            Err(err) => return Err(err),
        }
        if e.is_dir {
            // A folder cannot go into itself or a folder inside it.
            let mut up = new_parent;
            let mut depth = 0;
            while up != cnid::ROOT_PARENT {
                if up == e.cnid {
                    return Err(Error::Invalid);
                }
                depth += 1;
                if depth > 100_000 {
                    return Err(Error::Corrupt);
                }
                up = self.get(up).map_err(|_| Error::Corrupt)?.parent;
            }
        }
        self.delete_item(&e)?;
        e.parent = new_parent;
        e.name = new_name.to_vec();
        e.times[2] = now;
        self.insert_item(&e)?;
        if new_parent != parent {
            self.adjust_folder(parent, -1, now)?;
            self.adjust_folder(new_parent, 1, now)
        } else {
            self.adjust_folder(parent, 0, now)
        }
    }

    /// The root folder and its thread (mkfs).
    pub(crate) fn cat_make_root(&mut self, name: &[u8], now: u64) -> Result<(), Error> {
        let mut e = Entry::empty();
        e.cnid = cnid::ROOT;
        e.parent = cnid::ROOT_PARENT;
        e.is_dir = true;
        e.times[..4].fill(now);
        e.name = name.to_vec();
        self.insert_item(&e)
    }

    /// Every record of the catalog, in key order: (key, value).
    pub(crate) fn cat_records(&mut self) -> Result<Vec<Record>, Error> {
        let mut out = Vec::new();
        self.bt_walk(Special::Catalog, &[], |k, v| {
            out.push((k.to_vec(), v.to_vec()));
            false
        })?;
        Ok(out)
    }
}
