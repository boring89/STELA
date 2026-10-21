// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Mac's view of a STELA volume: [`Store`] over `libstela::fs`, from
//! the root folder's handle (`classic::hostfs` makes it a Mac volume).
//! Every call opens the items it needs by id and closes them again.

use abi::Error;
use abi::kind;
use alloc::string::String;
use alloc::vec::Vec;
use classic::hostfs::{Node, Store};
use libstela::Handle;
use libstela::fs::{self, open};
use libstela::ipc::handle_close;

/// A handle closed when dropped.
struct Owned(Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = handle_close(self.0);
    }
}

/// A volume's tree through its root handle (`READ | WRITE | MANAGE`).
pub struct DataStore {
    root: Handle,
    root_id: u64,
}

impl DataStore {
    /// The store of the folder `root` (kept open while the store lives).
    pub fn new(root: Handle) -> Result<DataStore, Error> {
        let root_id = fs::stat(root)?.id;
        Ok(DataStore { root, root_id })
    }

    fn open(&self, id: u64) -> Result<Owned, Error> {
        if id == self.root_id {
            return libstela::ipc::handle_duplicate(
                self.root,
                libstela::Rights::READ
                    | libstela::Rights::WRITE
                    | libstela::Rights::MANAGE
                    | libstela::Rights::DUPLICATE,
            )
            .map(Owned);
        }
        fs::open_by_id(self.root, id).map(Owned)
    }

    fn node(&self, h: Handle, name: String) -> Result<Node, Error> {
        let st = fs::stat(h)?;
        Ok(Node {
            id: st.id,
            is_dir: st.kind == kind::DIRECTORY,
            size: st.size,
            ctime: st.ctime,
            mtime: st.mtime,
            name,
        })
    }

    /// The last part of the item's path (its name).
    fn name_of(&self, h: Handle) -> Result<String, Error> {
        let path = fs::path_of(h)?;
        Ok(String::from(path.rsplit('/').next().unwrap_or("")))
    }
}

impl Drop for DataStore {
    fn drop(&mut self) {
        let _ = handle_close(self.root);
    }
}

impl Store for DataStore {
    fn root(&self) -> u64 {
        self.root_id
    }

    fn get(&mut self, id: u64) -> Result<Node, Error> {
        let h = self.open(id)?;
        let name = self.name_of(h.0)?;
        self.node(h.0, name)
    }

    fn parent(&mut self, id: u64) -> Result<u64, Error> {
        let h = self.open(id)?;
        let path = fs::path_of(h.0)?;
        let parent = match path.rsplit_once('/') {
            Some(("", _)) | None => return Ok(self.root_id),
            Some((p, _)) => p.trim_start_matches('/'),
        };
        let p = Owned(fs::open(self.root, parent, open::READ)?);
        Ok(fs::stat(p.0)?.id)
    }

    fn list(&mut self, dir: u64) -> Result<Vec<Node>, Error> {
        let d = self.open(dir)?;
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(e) = fs::read_dir(d.0, i)? {
            out.push(Node {
                id: e.id,
                is_dir: e.kind == kind::DIRECTORY,
                size: e.size,
                ctime: e.mtime,
                mtime: e.mtime,
                name: String::from(e.name()),
            });
            i += 1;
        }
        Ok(out)
    }

    fn lookup(&mut self, dir: u64, name: &str) -> Result<Node, Error> {
        if name.contains('/') {
            return Err(Error::NotFound);
        }
        let d = self.open(dir)?;
        let h = Owned(fs::open(d.0, name, open::READ)?);
        let name = self.name_of(h.0)?;
        self.node(h.0, name)
    }

    fn read(&mut self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let h = self.open(id)?;
        let mut done = 0;
        while done < buf.len() {
            match fs::read(h.0, &mut buf[done..], offset + done as u64)? {
                0 => break,
                n => done += n,
            }
        }
        Ok(done)
    }

    fn write(&mut self, id: u64, offset: u64, data: &[u8]) -> Result<usize, Error> {
        let h = self.open(id)?;
        let mut done = 0;
        while done < data.len() {
            match fs::write(h.0, &data[done..], offset + done as u64)? {
                0 => return Err(Error::IoError),
                n => done += n,
            }
        }
        Ok(done)
    }

    fn create(&mut self, dir: u64, name: &str, is_dir: bool) -> Result<Node, Error> {
        if name.contains('/') {
            return Err(Error::InvalidArgument);
        }
        let d = self.open(dir)?;
        let mut flags = open::READ | open::WRITE | open::CREATE | open::EXCLUSIVE;
        if is_dir {
            flags |= open::DIRECTORY;
        }
        let h = Owned(fs::open(d.0, name, flags)?);
        self.node(h.0, String::from(name))
    }

    fn remove(&mut self, dir: u64, name: &str) -> Result<(), Error> {
        let d = self.open(dir)?;
        fs::remove(d.0, name)
    }

    fn rename(&mut self, dir: u64, name: &str, new_dir: u64, new_name: &str) -> Result<(), Error> {
        if new_name.contains('/') {
            return Err(Error::InvalidArgument);
        }
        let from = self.open(dir)?;
        let to = self.open(new_dir)?;
        fs::rename(from.0, name, to.0, new_name)
    }

    fn set_size(&mut self, id: u64, size: u64) -> Result<(), Error> {
        let h = self.open(id)?;
        fs::set_size(h.0, size)
    }

    fn finder(&mut self, id: u64) -> Result<[u8; 32], Error> {
        let h = self.open(id)?;
        fs::get_info(h.0)
    }

    fn set_finder(&mut self, id: u64, info: &[u8; 32]) -> Result<(), Error> {
        let h = self.open(id)?;
        fs::set_info(h.0, info)
    }

    fn space(&mut self) -> (u64, u64) {
        let Ok(st) = fs::stat(self.root) else {
            return (0, 0);
        };
        match fs::volume_list(u64::from(st.volume)) {
            Ok(Some(v)) => (v.total, v.free),
            _ => (0, 0),
        }
    }
}
