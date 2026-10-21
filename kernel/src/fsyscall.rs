// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The file system calls (`docs/vfs.md`): folder and file handles, every
//! operation relative to a folder handle. Names and buffers are copied
//! through the caller's address space (checked page by page).

use crate::object::{Entry, KernelObject};
use crate::task::Process;
use crate::vfs::path::{components, single};
use crate::vfs::{self, Node, NodeInfo};
use abi::{DirEntry, Error, Handle, Rights, Stat, VolumeInfo, kind, open};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

/// Longest path `DirOpen` takes.
const PATH_MAX: u64 = 4096;
/// Bytes copied per step of `FileRead` and `FileWrite`.
const IO_CHUNK: usize = 64 * 1024;

fn user_bytes(process: &Process, ptr: u64, len: u64, max: u64) -> Result<Vec<u8>, Error> {
    if len > max {
        return Err(Error::InvalidArgument);
    }
    let mut buf = vec![0u8; len as usize];
    process
        .space()
        .read(ptr, &mut buf)
        .map_err(|_| Error::BadAddress)?;
    Ok(buf)
}

/// The folder or file behind `handle`: (node, is it a folder, rights).
fn node_of(process: &Process, handle: u64) -> Result<(Node, bool, Rights), Error> {
    let handles = process.handles().lock();
    let entry = handles.get(Handle::from_raw(handle))?;
    match &entry.object {
        KernelObject::Directory(n) => Ok((n.clone(), true, entry.rights)),
        KernelObject::File(n) => Ok((n.clone(), false, entry.rights)),
        _ => Err(Error::WrongType),
    }
}

/// The folder behind `handle`, which needs `rights`.
fn dir_of(process: &Process, handle: u64, rights: Rights) -> Result<(Node, Rights), Error> {
    let (node, is_dir, have) = node_of(process, handle)?;
    if !is_dir {
        return Err(Error::NotDir);
    }
    if !have.contains(rights) {
        return Err(Error::AccessDenied);
    }
    Ok((node, have))
}

/// The file behind `handle`, which needs `rights`.
fn file_of(process: &Process, handle: u64, rights: Rights) -> Result<Node, Error> {
    let (node, is_dir, have) = node_of(process, handle)?;
    if is_dir {
        return Err(Error::IsDir);
    }
    if !have.contains(rights) {
        return Err(Error::AccessDenied);
    }
    Ok(node)
}

fn insert(process: &Process, node: Node, is_dir: bool, rights: Rights) -> Result<u64, Error> {
    let object = if is_dir {
        KernelObject::Directory(node)
    } else {
        KernelObject::File(node)
    };
    let h = process.handles().lock().insert(Entry { object, rights })?;
    Ok(h.raw())
}

/// The rights a new handle gets: what was asked for, within what the
/// folder handle has, never WRITE on a read-only volume.
fn granted(asked: u64, parent: Rights, read_only: bool) -> Result<Rights, Error> {
    let mut rights = Rights::DUPLICATE | Rights::TRANSFER;
    if asked & open::READ != 0 || asked & open::WRITE == 0 {
        if !parent.contains(Rights::READ) {
            return Err(Error::AccessDenied);
        }
        rights = rights | Rights::READ;
    }
    if asked & open::WRITE != 0 {
        if read_only {
            return Err(Error::ReadOnly);
        }
        if !parent.contains(Rights::WRITE) {
            return Err(Error::AccessDenied);
        }
        rights = rights | Rights::WRITE;
    }
    // Metadata may be changed below a folder where it may be (P11).
    if parent.contains(Rights::MANAGE) && !read_only {
        rights = rights | Rights::MANAGE;
    }
    Ok(rights)
}

/// `DirOpen(dir, path_ptr, path_len, flags) -> handle`.
pub fn dir_open(process: &Process, dir: u64, ptr: u64, len: u64, flags: u64) -> Result<u64, Error> {
    if flags & !open::ALL != 0 {
        return Err(Error::InvalidArgument);
    }
    let (start, parent_rights) = dir_of(process, dir, Rights::READ)?;
    let path = user_bytes(process, ptr, len, PATH_MAX)?;
    let parts = components(&path).map_err(|_| Error::InvalidArgument)?;
    let fs = Arc::clone(&start.volume.fs);
    let mut folder = start.id;
    let Some((last, dirs)) = parts.split_last() else {
        // The folder itself, again.
        let rights = granted(flags, parent_rights, fs.read_only())?;
        return insert(process, start, true, rights);
    };
    for part in dirs {
        let info = fs.lookup(folder, part)?;
        if !info.is_dir {
            return Err(Error::NotDir);
        }
        folder = info.id;
    }
    let want_dir = flags & open::DIRECTORY != 0;
    let info: NodeInfo = match fs.lookup(folder, last) {
        Ok(info) => {
            if flags & open::CREATE != 0 && flags & open::EXCLUSIVE != 0 {
                return Err(Error::Exists);
            }
            if want_dir && !info.is_dir {
                return Err(Error::NotDir);
            }
            info
        }
        Err(Error::NotFound) if flags & open::CREATE != 0 => {
            if fs.read_only() {
                return Err(Error::ReadOnly);
            }
            if !parent_rights.contains(Rights::WRITE) {
                return Err(Error::AccessDenied);
            }
            fs.create(folder, last, want_dir)?
        }
        Err(e) => return Err(e),
    };
    let rights = granted(flags, parent_rights, fs.read_only())?;
    if flags & open::TRUNCATE != 0 && !info.is_dir {
        if !rights.contains(Rights::WRITE) {
            return Err(Error::AccessDenied);
        }
        fs.set_size(info.id, 0)?;
    }
    let node = Node {
        volume: start.volume,
        id: info.id,
    };
    insert(process, node, info.is_dir, rights)
}

/// `FileGetInfo(file or folder, &mut [u8; 32])`.
pub fn file_get_info(process: &Process, handle: u64, ptr: u64) -> Result<u64, Error> {
    let (node, _, _) = node_of(process, handle)?;
    let info = node.volume.fs.get_info(node.id)?;
    process
        .space()
        .write(ptr, &info)
        .map_err(|_| Error::BadAddress)?;
    Ok(0)
}

/// `FileSetInfo(file or folder, &[u8; 32])`: needs `WRITE`.
pub fn file_set_info(process: &Process, handle: u64, ptr: u64) -> Result<u64, Error> {
    let (node, _, rights) = node_of(process, handle)?;
    if !rights.contains(Rights::WRITE | Rights::MANAGE) {
        return Err(Error::AccessDenied);
    }
    let mut info = [0u8; abi::FINDER_INFO_SIZE];
    process
        .space()
        .read(ptr, &mut info)
        .map_err(|_| Error::BadAddress)?;
    if node.volume.fs.read_only() {
        return Err(Error::ReadOnly);
    }
    node.volume.fs.set_info(node.id, &info)?;
    Ok(0)
}

/// `DirRead(dir, index, &mut DirEntry) -> 1 | 0`.
pub fn dir_read(process: &Process, dir: u64, index: u64, ptr: u64) -> Result<u64, Error> {
    let (node, _) = dir_of(process, dir, Rights::READ)?;
    if !process.space().check(ptr, abi::DIR_ENTRY_SIZE, true) {
        return Err(Error::BadAddress);
    }
    let list = node.volume.fs.list(node.id)?;
    let Some(info) = usize::try_from(index).ok().and_then(|i| list.get(i)) else {
        return Ok(0);
    };
    let mut e = DirEntry::new();
    e.id = info.id;
    e.size = info.size;
    e.mtime = info.mtime;
    e.kind = if info.is_dir {
        kind::DIRECTORY
    } else {
        kind::FILE
    };
    let name = info.name.as_bytes();
    let n = name.len().min(abi::NAME_MAX);
    e.name[..n].copy_from_slice(&name[..n]);
    e.name_len = n as u32;
    process
        .space()
        .write(ptr, &e.to_bytes())
        .map_err(|_| Error::BadAddress)?;
    Ok(1)
}

/// `DirRemove(dir, name_ptr, name_len)`.
pub fn dir_remove(process: &Process, dir: u64, ptr: u64, len: u64) -> Result<u64, Error> {
    let (node, _) = dir_of(process, dir, Rights::WRITE)?;
    let name = user_bytes(process, ptr, len, abi::NAME_MAX as u64)?;
    let name = single(&name).map_err(|_| Error::InvalidArgument)?;
    node.volume.fs.remove(node.id, name)?;
    Ok(0)
}

/// `DirRename(dir, old_ptr, old_len, new_dir, new_ptr, new_len)`.
pub fn dir_rename(process: &Process, args: [u64; 6]) -> Result<u64, Error> {
    let [dir, old_ptr, old_len, new_dir, new_ptr, new_len] = args;
    let (from, _) = dir_of(process, dir, Rights::WRITE)?;
    let (to, _) = dir_of(process, new_dir, Rights::WRITE)?;
    if !Arc::ptr_eq(&from.volume, &to.volume) {
        return Err(Error::InvalidArgument);
    }
    let old = user_bytes(process, old_ptr, old_len, abi::NAME_MAX as u64)?;
    let new = user_bytes(process, new_ptr, new_len, abi::NAME_MAX as u64)?;
    let old = single(&old).map_err(|_| Error::InvalidArgument)?;
    let new = single(&new).map_err(|_| Error::InvalidArgument)?;
    from.volume.fs.rename(from.id, old, to.id, new)?;
    Ok(0)
}

/// `FileRead(file, buf, len, offset) -> bytes`.
pub fn file_read(
    process: &Process,
    file: u64,
    ptr: u64,
    len: u64,
    offset: u64,
) -> Result<u64, Error> {
    let node = file_of(process, file, Rights::READ)?;
    if !process.space().check(ptr, len as usize, true) {
        return Err(Error::BadAddress);
    }
    let mut buf = vec![0u8; (len as usize).min(IO_CHUNK)];
    let mut done = 0u64;
    while done < len {
        let want = ((len - done) as usize).min(IO_CHUNK);
        let n = node
            .volume
            .fs
            .read(node.id, offset.saturating_add(done), &mut buf[..want])?;
        if n == 0 {
            break;
        }
        process
            .space()
            .write(ptr + done, &buf[..n])
            .map_err(|_| Error::BadAddress)?;
        done += n as u64;
        if n < want {
            break;
        }
    }
    Ok(done)
}

/// `FileWrite(file, buf, len, offset) -> bytes`.
pub fn file_write(
    process: &Process,
    file: u64,
    ptr: u64,
    len: u64,
    offset: u64,
) -> Result<u64, Error> {
    let node = file_of(process, file, Rights::WRITE)?;
    if offset.checked_add(len).is_none() {
        return Err(Error::InvalidArgument);
    }
    let mut done = 0u64;
    let mut buf = vec![0u8; (len as usize).min(IO_CHUNK)];
    while done < len {
        let n = ((len - done) as usize).min(IO_CHUNK);
        process
            .space()
            .read(ptr + done, &mut buf[..n])
            .map_err(|_| Error::BadAddress)?;
        node.volume.fs.write(node.id, offset + done, &buf[..n])?;
        done += n as u64;
    }
    Ok(done)
}

/// `FileStat(file_or_dir, &mut Stat)`.
pub fn file_stat(process: &Process, h: u64, ptr: u64) -> Result<u64, Error> {
    let (node, _, _) = node_of(process, h)?;
    let info = node.volume.fs.get(node.id)?;
    let stat = Stat {
        id: info.id,
        size: info.size,
        ctime: info.ctime,
        mtime: info.mtime,
        kind: if info.is_dir {
            kind::DIRECTORY
        } else {
            kind::FILE
        },
        volume: node.volume.index,
        flags: u32::from(node.volume.fs.read_only()),
        reserved: 0,
    };
    process
        .space()
        .write(ptr, &stat.to_bytes())
        .map_err(|_| Error::BadAddress)?;
    Ok(0)
}

/// `FilePath(item, buf, len) -> length`: the path inside the volume.
pub fn file_path(process: &Process, h: u64, ptr: u64, len: u64) -> Result<u64, Error> {
    let (node, _, rights) = node_of(process, h)?;
    if !rights.contains(Rights::READ) {
        return Err(Error::AccessDenied);
    }
    let path = vfs::path_of(&node.volume, node.id)?;
    let n = path.len().min(len as usize);
    process
        .space()
        .write(ptr, &path.as_bytes()[..n])
        .map_err(|_| Error::BadAddress)?;
    Ok(path.len() as u64)
}

/// `FileSetSize(file, size)`.
pub fn file_set_size(process: &Process, file: u64, size: u64) -> Result<u64, Error> {
    let node = file_of(process, file, Rights::WRITE)?;
    node.volume.fs.set_size(node.id, size)?;
    Ok(0)
}

/// The record `VolumeList` returns for `volume`.
pub fn volume_info(volume: &vfs::Volume) -> VolumeInfo {
    let (total, free) = volume.fs.space();
    let mut info = VolumeInfo::new();
    info.index = volume.index;
    info.flags = u32::from(volume.fs.read_only());
    info.kind = volume.fs.kind();
    info.total = total;
    info.free = free;
    let name = volume.name.as_bytes();
    let n = name.len().min(64);
    info.name[..n].copy_from_slice(&name[..n]);
    info.name_len = n as u32;
    info
}

/// `VolumeList(index, &mut VolumeInfo) -> 1 | 0`.
pub fn volume_list(process: &Process, index: u64, ptr: u64) -> Result<u64, Error> {
    let volumes = vfs::volumes();
    let Some(volume) = usize::try_from(index).ok().and_then(|i| volumes.get(i)) else {
        return Ok(0);
    };
    process
        .space()
        .write(ptr, &volume_info(volume).to_bytes())
        .map_err(|_| Error::BadAddress)?;
    Ok(1)
}

/// The rights of a volume's root handle.
pub fn root_rights(volume: &vfs::Volume) -> Rights {
    let mut rights = Rights::READ | Rights::DUPLICATE | Rights::TRANSFER;
    if !volume.fs.read_only() {
        rights = rights | Rights::WRITE | Rights::MANAGE;
    }
    rights
}

/// `VolumeOpen(name_ptr, name_len) -> handle`.
pub fn volume_open(process: &Process, ptr: u64, len: u64) -> Result<u64, Error> {
    let name = user_bytes(process, ptr, len, 64)?;
    let name = core::str::from_utf8(&name).map_err(|_| Error::InvalidArgument)?;
    let volume = vfs::find(name).ok_or(Error::NotFound)?;
    let rights = root_rights(&volume);
    let id = volume.fs.root();
    insert(process, Node { volume, id }, true, rights)
}

/// `VolumeOpenById(dir, id) -> handle`.
pub fn volume_open_by_id(process: &Process, dir: u64, id: u64) -> Result<u64, Error> {
    let (node, rights) = dir_of(process, dir, Rights::READ)?;
    let info = node.volume.fs.get(id)?;
    let rights =
        Rights(rights.0 & (Rights::READ | Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER).0);
    insert(
        process,
        Node {
            volume: node.volume,
            id: info.id,
        },
        info.is_dir,
        rights,
    )
}
