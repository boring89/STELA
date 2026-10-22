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

/// `DirRead(dir, cookie, &mut DirEntry) -> next cookie | 0`.
pub fn dir_read(process: &Process, dir: u64, cookie: u64, ptr: u64) -> Result<u64, Error> {
    let (node, _) = dir_of(process, dir, Rights::READ)?;
    if !process.space().check(ptr, abi::DIR_ENTRY_SIZE, true) {
        return Err(Error::BadAddress);
    }
    let Some((info, next)) = node.volume.fs.list_after(node.id, cookie)? else {
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
    Ok(next)
}

/// `DirRemove(dir, name_ptr, name_len)`.
pub fn dir_remove(process: &Process, dir: u64, ptr: u64, len: u64) -> Result<u64, Error> {
    let (node, _) = dir_of(process, dir, Rights::WRITE)?;
    let name = user_bytes(process, ptr, len, abi::NAME_MAX as u64)?;
    let name = single(&name).map_err(|_| Error::InvalidArgument)?;
    node.volume.fs.check_unlocked(node.id, name)?;
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
    from.volume.fs.check_unlocked(from.id, old)?;
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

/// The name of an extended attribute in the caller's memory.
fn attr_name(process: &Process, ptr: u64, len: u64) -> Result<alloc::string::String, Error> {
    if len == 0 || len as usize > abi::ATTR_NAME_MAX {
        return Err(Error::InvalidArgument);
    }
    let bytes = user_bytes(process, ptr, len, abi::ATTR_NAME_MAX as u64)?;
    let name = alloc::string::String::from_utf8(bytes).map_err(|_| Error::InvalidArgument)?;
    if name.contains('\0') {
        return Err(Error::InvalidArgument);
    }
    Ok(name)
}

/// `FileGetAttr(item, name, name_len, buf, len) -> length`.
pub fn file_get_attr(process: &Process, args: [u64; 6]) -> Result<u64, Error> {
    let [h, name_ptr, name_len, buf, len, _] = args;
    let (node, _, rights) = node_of(process, h)?;
    if !rights.contains(Rights::READ) {
        return Err(Error::AccessDenied);
    }
    let name = attr_name(process, name_ptr, name_len)?;
    let value = node.volume.fs.get_attr(node.id, &name)?;
    let n = value.len().min(len as usize);
    process
        .space()
        .write(buf, &value[..n])
        .map_err(|_| Error::BadAddress)?;
    Ok(value.len() as u64)
}

/// `FileSetAttr(item, name, name_len, value, len)`: `len` 0 removes it.
pub fn file_set_attr(process: &Process, args: [u64; 6]) -> Result<u64, Error> {
    let [h, name_ptr, name_len, buf, len, _] = args;
    let (node, _, rights) = node_of(process, h)?;
    let name = attr_name(process, name_ptr, name_len)?;
    may_set_attr(process, rights, &name)?;
    if node.volume.fs.read_only() {
        return Err(Error::ReadOnly);
    }
    if len as usize > abi::ATTR_VALUE_MAX {
        return Err(Error::InvalidArgument);
    }
    if len == 0 {
        node.volume.fs.set_attr(node.id, &name, None)?;
    } else {
        let value = user_bytes(process, buf, len, abi::ATTR_VALUE_MAX as u64)?;
        node.volume.fs.set_attr(node.id, &name, Some(&value))?;
    }
    Ok(0)
}

/// Who may write attribute `name` (P14 K3, `docs/vfs.md` §4.5):
/// `stela.OWNER[.…]` only the program OWNER (by its bundle or file name,
/// ignoring case; when `Initrd` has a program of that name, only that
/// copy) with `WRITE`; any other name needs `WRITE` and `MANAGE`.
fn may_set_attr(process: &Process, rights: Rights, name: &str) -> Result<(), Error> {
    let Some(owner) = abi::attr::owner(name) else {
        return if rights.contains(Rights::WRITE | Rights::MANAGE) {
            Ok(())
        } else {
            Err(Error::AccessDenied)
        };
    };
    if !rights.contains(Rights::WRITE) {
        return Err(Error::AccessDenied);
    }
    let Some((program, system)) = process.program() else {
        return Err(Error::AccessDenied);
    };
    if !program.eq_ignore_ascii_case(owner) {
        return Err(Error::AccessDenied);
    }
    if !system && system_has_program(owner) {
        return Err(Error::AccessDenied);
    }
    Ok(())
}

/// Whether the system has a program `name` (a file or `name.app`,
/// ignoring case) in `Initrd:/` or (P14 K4) `System:/Programs/`.
fn system_has_program(name: &str) -> bool {
    let in_folder = |volume: &str, folder: Option<&str>| {
        let Some(v) = vfs::find(volume) else {
            return false;
        };
        let mut dir = v.fs.root();
        if let Some(f) = folder {
            match v.fs.lookup(dir, f) {
                Ok(i) => dir = i.id,
                Err(_) => return false,
            }
        }
        v.fs.list(dir).is_ok_and(|items| has_program(&items, name))
    };
    in_folder("Initrd", None) || in_folder(vfs::mount::SYSTEM, Some("Programs"))
}

/// Whether `items` has a program `name` (a file or `name.app`).
fn has_program(items: &[vfs::NodeInfo], name: &str) -> bool {
    items.iter().any(|i| {
        let n = i.name.as_str();
        n.eq_ignore_ascii_case(name)
            || n.len() == name.len() + 4
                && n[..name.len()].eq_ignore_ascii_case(name)
                && n[name.len()..].eq_ignore_ascii_case(".app")
    })
}

/// `FileListAttr(item, buf, len) -> length`: names ending in NUL.
pub fn file_list_attr(process: &Process, h: u64, buf: u64, len: u64) -> Result<u64, Error> {
    let (node, _, rights) = node_of(process, h)?;
    if !rights.contains(Rights::READ) {
        return Err(Error::AccessDenied);
    }
    let mut out = alloc::vec::Vec::new();
    for name in node.volume.fs.list_attrs(node.id)? {
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    let n = out.len().min(len as usize);
    process
        .space()
        .write(buf, &out[..n])
        .map_err(|_| Error::BadAddress)?;
    Ok(out.len() as u64)
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

/// `VolumeList(index, &mut VolumeInfo, volumes) -> 1 | 0` (P13 K15: needs
/// `VOLUMES`).
pub fn volume_list(process: &Process, index: u64, ptr: u64, cap: u64) -> Result<u64, Error> {
    crate::object::check_capability(process, cap, abi::capability::VOLUMES)?;
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

/// `VolumeOpen(name_ptr, name_len, volumes) -> handle` (P13 K15: needs
/// `VOLUMES`, except for `Initrd` and (P14 K4) `System`: the read-only
/// system files, fonts and certificates every program may read;
/// `docs/sandbox.md`).
pub fn volume_open(process: &Process, ptr: u64, len: u64, cap: u64) -> Result<u64, Error> {
    let name = user_bytes(process, ptr, len, 64)?;
    let name = core::str::from_utf8(&name).map_err(|_| Error::InvalidArgument)?;
    if name != "Initrd" && name != vfs::mount::SYSTEM {
        crate::object::check_capability(process, cap, abi::capability::VOLUMES)?;
    }
    let volume = vfs::find(name).ok_or(Error::NotFound)?;
    let rights = root_rights(&volume);
    let id = volume.fs.root();
    insert(process, Node { volume, id }, true, rights)
}

/// `VolumeOpenById(dir, id, volumes) -> handle` (P13 K15: needs `VOLUMES`;
/// an id reaches anywhere on the volume, past the folder `dir` is).
pub fn volume_open_by_id(process: &Process, dir: u64, id: u64, cap: u64) -> Result<u64, Error> {
    crate::object::check_capability(process, cap, abi::capability::VOLUMES)?;
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
