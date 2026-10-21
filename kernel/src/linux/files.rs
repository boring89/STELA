// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files for Linux programs: descriptors over ttys, devices and the
//! volumes, reached through the synthetic root (`path.rs`): `/Data/a.txt`
//! is `a.txt` on the `Data` volume, `/tmp` the `Tmp` volume, `/dev` the
//! devices.

use super::errno::{
    self, EBADF, EEXIST, EINVAL, EISDIR, EMFILE, ENAMETOOLONG, ENOENT, ENOTDIR, ENOTEMPTY, ERANGE,
    EROFS, ESPIPE, EXDEV,
};
use super::fd::FdError;
use super::path::{self, Config, Device, PathError, Target};
use super::state::{FileState, OpenFile};
use super::sys::{LinuxResult, file_of, read_user, write_user};
use crate::task::Process;
use crate::vfs::{self, Node, NodeInfo, Volume};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use frame::sync::SpinLock;

/// `dirfd` meaning "the working directory".
pub const AT_FDCWD: u64 = (-100i64) as u64;
/// `unlinkat` flag: remove a folder.
pub const AT_REMOVEDIR: u64 = 0x200;

const O_ACCMODE: u64 = 3;
const O_WRONLY: u64 = 1;
const O_RDWR: u64 = 2;
const O_CREAT: u64 = 0o100;
const O_EXCL: u64 = 0o200;
const O_TRUNC: u64 = 0o1000;
const O_APPEND: u64 = 0o2000;
const O_DIRECTORY: u64 = 0o200000;
const O_CLOEXEC: u64 = 0o2000000;

/// Bytes moved per step of a read or write.
const CHUNK: usize = 64 * 1024;

fn fd_error(e: FdError) -> i64 {
    match e {
        FdError::BadFd => EBADF,
        FdError::Full => EMFILE,
    }
}

/// A NUL-terminated string of the caller (at most `PATH_MAX` bytes).
fn user_string(p: &Process, ptr: u64) -> Result<String, i64> {
    let mut out = Vec::new();
    let mut at = ptr;
    loop {
        // Up to the end of the page, so no unmapped page is touched.
        let n = (4096 - (at & 0xFFF)) as usize;
        let chunk = read_user(p, at, n)?;
        if let Some(end) = chunk.iter().position(|&b| b == 0) {
            out.extend_from_slice(&chunk[..end]);
            break;
        }
        out.extend_from_slice(&chunk);
        if out.len() > path::PATH_MAX {
            return Err(ENAMETOOLONG);
        }
        at += n as u64;
    }
    String::from_utf8(out).map_err(|_| EINVAL)
}

/// The Linux path of the folder `dirfd` names (or the working directory).
fn base_of(p: &Process, dirfd: u64) -> Result<String, i64> {
    let linux = p.linux().ok_or(EBADF)?;
    if dirfd == AT_FDCWD {
        return Ok(linux.cwd.lock().clone());
    }
    match &*file_of(p, dirfd)? {
        OpenFile::Dir { path, .. } => Ok(path.clone()),
        _ => Err(ENOTDIR),
    }
}

/// The normalised components of the caller's path `ptr` relative to
/// `dirfd`.
fn parts_of(p: &Process, dirfd: u64, ptr: u64) -> Result<Vec<String>, i64> {
    let path = user_string(p, ptr)?;
    let base = base_of(p, dirfd)?;
    path::normalize(&base, &path).map_err(|e| match e {
        PathError::Empty => ENOENT,
        PathError::TooLong => ENAMETOOLONG,
    })
}

/// What a path resolved to.
enum Found {
    /// The synthetic root.
    Root,
    /// `/dev`.
    DevDir,
    /// A device.
    Device(Device),
    /// `/etc`.
    EtcDir,
    /// A generated file in `/etc`.
    Config(Config),
    /// An item on a volume.
    Item(Node, NodeInfo),
}

/// The contents of a generated file.
fn config_text(c: Config) -> Vec<u8> {
    match c {
        Config::ResolvConf => match crate::net::dns_server() {
            Some([a, b, c, d]) => alloc::format!("nameserver {a}.{b}.{c}.{d}\n").into_bytes(),
            None => Vec::new(),
        },
    }
}

fn volume(name: &str) -> Result<Arc<Volume>, i64> {
    vfs::find(name).ok_or(ENOENT)
}

/// Walks `parts` from the root of `volume`.
fn walk(volume: &Arc<Volume>, parts: &[String]) -> Result<NodeInfo, i64> {
    let fs = &volume.fs;
    let mut info = fs.get(fs.root()).map_err(errno::from_error)?;
    for part in parts {
        if !info.is_dir {
            return Err(ENOTDIR);
        }
        info = fs.lookup(info.id, part).map_err(errno::from_error)?;
    }
    Ok(info)
}

fn resolve(parts: &[String]) -> Result<Found, i64> {
    match path::target(parts) {
        Target::Root => Ok(Found::Root),
        Target::DevDir => Ok(Found::DevDir),
        Target::Device(d) => Ok(Found::Device(d)),
        Target::EtcDir => Ok(Found::EtcDir),
        Target::Config(c) => Ok(Found::Config(c)),
        Target::Volume(name, rest) => {
            let volume = volume(&name)?;
            let info = walk(&volume, &rest)?;
            Ok(Found::Item(
                Node {
                    volume,
                    id: info.id,
                },
                info,
            ))
        }
    }
}

/// The parent folder (on a volume) and the last name of `parts`.
fn parent_of(parts: &[String]) -> Result<(Node, String), i64> {
    let (parent, name) = path::split_last(parts).ok_or(EEXIST)?;
    match resolve(parent)? {
        Found::Item(node, info) if info.is_dir => Ok((node, String::from(name))),
        Found::Item(..) => Err(ENOTDIR),
        // New names at the root or in /dev cannot be made.
        _ => Err(errno::EACCES),
    }
}

pub(super) fn add_fd(p: &Process, file: OpenFile, cloexec: bool) -> LinuxResult {
    let linux = p.linux().ok_or(EBADF)?;
    let fd = linux
        .fds
        .lock()
        .insert_from(0, Arc::new(file), cloexec)
        .map_err(fd_error)?;
    Ok(fd as u64)
}

/// `openat(dirfd, path, flags, mode)` (and `open`).
pub fn openat(p: &Process, dirfd: u64, ptr: u64, flags: u64) -> LinuxResult {
    let parts = parts_of(p, dirfd, ptr)?;
    let access = flags & O_ACCMODE;
    let writing = access == O_WRONLY || access == O_RDWR;
    let cloexec = flags & O_CLOEXEC != 0;
    let dir = |node: Option<Node>| OpenFile::Dir {
        node,
        path: path::join(&parts),
        cursor: SpinLock::new(0),
    };
    let found = match resolve(&parts) {
        Ok(found) => {
            if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                return Err(EEXIST);
            }
            found
        }
        Err(ENOENT) if flags & O_CREAT != 0 => {
            let (parent, name) = parent_of(&parts)?;
            let fs = &parent.volume.fs;
            if fs.read_only() {
                return Err(EROFS);
            }
            let info = fs
                .create(parent.id, &name, false)
                .map_err(errno::from_error)?;
            Found::Item(
                Node {
                    volume: parent.volume,
                    id: info.id,
                },
                info,
            )
        }
        Err(e) => return Err(e),
    };
    let file = match found {
        Found::Root | Found::DevDir | Found::EtcDir if writing => return Err(EISDIR),
        Found::Root | Found::DevDir | Found::EtcDir => dir(None),
        Found::Config(_) if flags & O_DIRECTORY != 0 => return Err(ENOTDIR),
        Found::Config(_) if writing => return Err(EROFS),
        Found::Config(c) => OpenFile::Text {
            data: config_text(c),
            offset: SpinLock::new(0),
        },
        Found::Device(_) if flags & O_DIRECTORY != 0 => return Err(ENOTDIR),
        Found::Device(Device::Null) => OpenFile::Null,
        Found::Device(Device::Zero) => OpenFile::Zero,
        Found::Device(Device::Tty) => OpenFile::Tty(Arc::clone(&p.linux().ok_or(EBADF)?.tty)),
        Found::Item(node, info) if info.is_dir => {
            if writing {
                return Err(EISDIR);
            }
            dir(Some(node))
        }
        Found::Item(_, _) if flags & O_DIRECTORY != 0 => return Err(ENOTDIR),
        Found::Item(node, info) => {
            if writing && node.volume.fs.read_only() {
                return Err(EROFS);
            }
            if flags & O_TRUNC != 0 && writing {
                node.volume
                    .fs
                    .set_size(info.id, 0)
                    .map_err(errno::from_error)?;
            }
            OpenFile::File {
                node,
                state: SpinLock::new(FileState {
                    offset: 0,
                    append: flags & O_APPEND != 0,
                    readable: access != O_WRONLY,
                    writable: writing,
                    flags: flags & !O_CLOEXEC,
                }),
            }
        }
    };
    add_fd(p, file, cloexec)
}

/// `close(fd)`.
pub fn close(p: &Process, fd: u64) -> LinuxResult {
    let linux = p.linux().ok_or(EBADF)?;
    let file = linux.fds.lock().remove(fd).map_err(fd_error)?;
    drop(file);
    Ok(0)
}

/// `dup(fd)`.
pub fn dup(p: &Process, fd: u64) -> LinuxResult {
    let linux = p.linux().ok_or(EBADF)?;
    let new = linux.fds.lock().dup(fd).map_err(fd_error)?;
    Ok(new as u64)
}

/// `dup2(fd, to)`.
pub fn dup2(p: &Process, fd: u64, to: u64) -> LinuxResult {
    let linux = p.linux().ok_or(EBADF)?;
    let (new, old) = linux.fds.lock().dup2(fd, to).map_err(fd_error)?;
    drop(old);
    Ok(new as u64)
}

/// `fcntl(fd, cmd, arg)`: descriptor flags, status flags, duplication.
pub fn fcntl(p: &Process, fd: u64, cmd: u64, arg: u64) -> LinuxResult {
    const F_DUPFD: u64 = 0;
    const F_GETFD: u64 = 1;
    const F_SETFD: u64 = 2;
    const F_GETFL: u64 = 3;
    const F_SETFL: u64 = 4;
    const F_DUPFD_CLOEXEC: u64 = 1030;
    const FD_CLOEXEC: u64 = 1;
    let linux = p.linux().ok_or(EBADF)?;
    let mut fds = linux.fds.lock();
    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let file = Arc::clone(&fds.get(fd).map_err(fd_error)?.file);
            let new = fds
                .insert_from(arg as usize, file, cmd == F_DUPFD_CLOEXEC)
                .map_err(fd_error)?;
            Ok(new as u64)
        }
        F_GETFD => Ok(u64::from(fds.get(fd).map_err(fd_error)?.cloexec)),
        F_SETFD => {
            fds.get_mut(fd).map_err(fd_error)?.cloexec = arg & FD_CLOEXEC != 0;
            Ok(0)
        }
        F_GETFL => {
            let file = Arc::clone(&fds.get(fd).map_err(fd_error)?.file);
            drop(fds);
            Ok(match &*file {
                OpenFile::File { state, .. } => state.lock().flags,
                OpenFile::Dir { .. } => O_DIRECTORY,
                _ => O_RDWR,
            })
        }
        F_SETFL => {
            let file = Arc::clone(&fds.get(fd).map_err(fd_error)?.file);
            drop(fds);
            if let OpenFile::File { state, .. } = &*file {
                let mut s = state.lock();
                s.append = arg & O_APPEND != 0;
                s.flags = (s.flags & !O_APPEND) | (arg & O_APPEND);
            }
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

/// Reads up to `len` bytes from `file` into a buffer.
fn read_file(file: &OpenFile, len: usize) -> Result<Vec<u8>, i64> {
    let len = len.min(CHUNK);
    match file {
        OpenFile::Tty(tty) => tty.read(len, None).map_err(errno::from_error),
        OpenFile::Null => Ok(Vec::new()),
        OpenFile::Zero => Ok(vec![0u8; len]),
        OpenFile::Dir { .. } => Err(EISDIR),
        OpenFile::Socket(socket) => super::socket::read(socket, len),
        OpenFile::Text { data, offset } => {
            let mut at = offset.lock();
            let start = (*at as usize).min(data.len());
            let end = (start + len).min(data.len());
            *at = end as u64;
            Ok(data[start..end].to_vec())
        }
        OpenFile::File { node, state } => {
            let mut s = state.lock();
            if !s.readable {
                return Err(EBADF);
            }
            let mut buf = vec![0u8; len];
            let n = node
                .volume
                .fs
                .read(node.id, s.offset, &mut buf)
                .map_err(errno::from_error)?;
            buf.truncate(n);
            s.offset += n as u64;
            Ok(buf)
        }
    }
}

/// Writes `data` to `file`; returns how much was written.
fn write_file(file: &OpenFile, data: &[u8]) -> Result<usize, i64> {
    match file {
        OpenFile::Tty(tty) => {
            tty.write(data);
            Ok(data.len())
        }
        OpenFile::Null | OpenFile::Zero => Ok(data.len()),
        OpenFile::Dir { .. } => Err(EISDIR),
        OpenFile::Socket(socket) => super::socket::write(socket, data),
        OpenFile::Text { .. } => Err(EBADF),
        OpenFile::File { node, state } => {
            let mut s = state.lock();
            if !s.writable {
                return Err(EBADF);
            }
            let fs = &node.volume.fs;
            if s.append {
                s.offset = fs.get(node.id).map_err(errno::from_error)?.size;
            }
            fs.write(node.id, s.offset, data)
                .map_err(errno::from_error)?;
            s.offset += data.len() as u64;
            Ok(data.len())
        }
    }
}

/// `read(fd, buf, len)`.
pub fn read(p: &Process, fd: u64, buf: u64, len: u64) -> LinuxResult {
    let file = file_of(p, fd)?;
    if !p.space().check(buf, len.min(CHUNK as u64) as usize, true) {
        return Err(errno::EFAULT);
    }
    let data = read_file(&file, len as usize)?;
    write_user(p, buf, &data)?;
    Ok(data.len() as u64)
}

/// `write(fd, buf, len)`.
pub fn write(p: &Process, fd: u64, buf: u64, len: u64) -> LinuxResult {
    let file = file_of(p, fd)?;
    let mut done = 0u64;
    while done < len {
        let n = (len - done).min(CHUNK as u64) as usize;
        let data = read_user(p, buf + done, n)?;
        write_file(&file, &data)?;
        done += n as u64;
    }
    Ok(done)
}

/// The (base, len) pairs of an iovec array.
fn iovecs(p: &Process, iov: u64, count: u64) -> Result<Vec<(u64, u64)>, i64> {
    if count > 1024 {
        return Err(EINVAL);
    }
    let raw = read_user(p, iov, count as usize * 16)?;
    Ok(raw
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| {
            let mut a = [0u8; 8];
            let mut b = [0u8; 8];
            a.copy_from_slice(&c[..8]);
            b.copy_from_slice(&c[8..]);
            (u64::from_le_bytes(a), u64::from_le_bytes(b))
        })
        .collect())
}

/// `writev(fd, iov, count)`.
pub fn writev(p: &Process, fd: u64, iov: u64, count: u64) -> LinuxResult {
    let mut total = 0;
    for (base, len) in iovecs(p, iov, count)? {
        total += write(p, fd, base, len)?;
    }
    Ok(total)
}

/// `readv(fd, iov, count)`: one read, spread over the buffers.
pub fn readv(p: &Process, fd: u64, iov: u64, count: u64) -> LinuxResult {
    let vecs = iovecs(p, iov, count)?;
    let total: u64 = vecs.iter().map(|v| v.1).sum();
    let file = file_of(p, fd)?;
    let data = read_file(&file, total as usize)?;
    let mut at = 0usize;
    for (base, len) in vecs {
        if at >= data.len() {
            break;
        }
        let n = (len as usize).min(data.len() - at);
        write_user(p, base, &data[at..at + n])?;
        at += n;
    }
    Ok(data.len() as u64)
}

/// `lseek(fd, offset, whence)`.
pub fn lseek(p: &Process, fd: u64, offset: i64, whence: u64) -> LinuxResult {
    const SEEK_SET: u64 = 0;
    const SEEK_CUR: u64 = 1;
    const SEEK_END: u64 = 2;
    let file = file_of(p, fd)?;
    match &*file {
        OpenFile::File { node, state } => {
            let mut s = state.lock();
            let base = match whence {
                SEEK_SET => 0,
                SEEK_CUR => s.offset as i64,
                SEEK_END => node.volume.fs.get(node.id).map_err(errno::from_error)?.size as i64,
                _ => return Err(EINVAL),
            };
            let new = base.checked_add(offset).filter(|&o| o >= 0).ok_or(EINVAL)?;
            s.offset = new as u64;
            Ok(new as u64)
        }
        OpenFile::Dir { cursor, .. } if whence == SEEK_SET && offset == 0 => {
            *cursor.lock() = 0;
            Ok(0)
        }
        OpenFile::Text { data, offset: at } => {
            let mut at = at.lock();
            let base = match whence {
                SEEK_SET => 0,
                SEEK_CUR => *at as i64,
                SEEK_END => data.len() as i64,
                _ => return Err(EINVAL),
            };
            let new = base.checked_add(offset).filter(|&o| o >= 0).ok_or(EINVAL)?;
            *at = new as u64;
            Ok(new as u64)
        }
        OpenFile::Null | OpenFile::Zero => Ok(0),
        _ => Err(ESPIPE),
    }
}

/// File types and permissions of `st_mode`.
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFCHR: u32 = 0o020000;
const S_IFSOCK: u32 = 0o140000;

/// The kernel's `struct stat` on x86-64 (144 bytes).
fn stat_bytes(dev: u64, ino: u64, mode: u32, size: u64, ctime: u64, mtime: u64) -> [u8; 144] {
    let mut b = [0u8; 144];
    let put =
        |b: &mut [u8; 144], at: usize, v: u64| b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    put(&mut b, 0, dev);
    put(&mut b, 8, ino);
    put(&mut b, 16, 1); // st_nlink
    b[24..28].copy_from_slice(&mode.to_le_bytes());
    put(&mut b, 48, size);
    put(&mut b, 56, 4096); // st_blksize
    put(&mut b, 64, size.div_ceil(512)); // st_blocks
    for (at, t) in [(72, mtime), (88, mtime), (104, ctime)] {
        put(&mut b, at, t / 1_000_000_000);
        put(&mut b, at + 8, t % 1_000_000_000);
    }
    b
}

fn stat_of(found: &Found) -> [u8; 144] {
    match found {
        Found::Root => stat_bytes(0, 1, S_IFDIR | 0o755, 0, 0, 0),
        Found::DevDir => stat_bytes(0, 2, S_IFDIR | 0o755, 0, 0, 0),
        Found::EtcDir => stat_bytes(0, 8, S_IFDIR | 0o755, 0, 0, 0),
        Found::Config(c) => stat_bytes(
            0,
            9 + *c as u64,
            S_IFREG | 0o444,
            config_text(*c).len() as u64,
            0,
            0,
        ),
        Found::Device(d) => stat_bytes(0, 3 + *d as u64, S_IFCHR | 0o666, 0, 0, 0),
        Found::Item(node, info) => {
            let mode = if info.is_dir {
                S_IFDIR | 0o755
            } else {
                S_IFREG | 0o644
            };
            let dev = u64::from(node.volume.index) + 1;
            stat_bytes(dev, info.id, mode, info.size, info.ctime, info.mtime)
        }
    }
}

/// `newfstatat(dirfd, path, buf, flags)` (and `stat`, `lstat`).
pub fn stat_path(p: &Process, dirfd: u64, ptr: u64, buf: u64, flags: u64) -> LinuxResult {
    const AT_EMPTY_PATH: u64 = 0x1000;
    if flags & AT_EMPTY_PATH != 0 && user_string(p, ptr)?.is_empty() {
        return fstat(p, dirfd, buf);
    }
    let parts = parts_of(p, dirfd, ptr)?;
    let found = resolve(&parts)?;
    write_user(p, buf, &stat_of(&found))?;
    Ok(0)
}

/// `fstat(fd, buf)`.
pub fn fstat(p: &Process, fd: u64, buf: u64) -> LinuxResult {
    let file = file_of(p, fd)?;
    let bytes = match &*file {
        OpenFile::Tty(_) => stat_bytes(0, 6, S_IFCHR | 0o620, 0, 0, 0),
        OpenFile::Null => stat_bytes(0, 3, S_IFCHR | 0o666, 0, 0, 0),
        OpenFile::Zero => stat_bytes(0, 4, S_IFCHR | 0o666, 0, 0, 0),
        OpenFile::Socket(_) => stat_bytes(0, 7, S_IFSOCK | 0o777, 0, 0, 0),
        OpenFile::Text { data, .. } => stat_bytes(0, 9, S_IFREG | 0o444, data.len() as u64, 0, 0),
        OpenFile::File { node, .. }
        | OpenFile::Dir {
            node: Some(node), ..
        } => {
            let info = node.volume.fs.get(node.id).map_err(errno::from_error)?;
            stat_of(&Found::Item(node.clone(), info))
        }
        OpenFile::Dir { path, .. } if path == "/dev" => stat_of(&Found::DevDir),
        OpenFile::Dir { path, .. } if path == "/etc" => stat_of(&Found::EtcDir),
        OpenFile::Dir { .. } => stat_of(&Found::Root),
    };
    write_user(p, buf, &bytes)?;
    Ok(0)
}

/// `faccessat(dirfd, path, mode)` (and `access`): it exists, and is
/// writable unless on a read-only volume.
pub fn access(p: &Process, dirfd: u64, ptr: u64, mode: u64) -> LinuxResult {
    const W_OK: u64 = 2;
    let parts = parts_of(p, dirfd, ptr)?;
    let found = resolve(&parts)?;
    if mode & W_OK != 0 {
        match &found {
            Found::Item(node, _) if node.volume.fs.read_only() => return Err(EROFS),
            Found::Config(_) => return Err(EROFS),
            _ => {}
        }
    }
    Ok(0)
}

/// `getcwd(buf, size)`: returns the length including the NUL.
pub fn getcwd(p: &Process, buf: u64, size: u64) -> LinuxResult {
    let linux = p.linux().ok_or(EBADF)?;
    let mut cwd = linux.cwd.lock().clone().into_bytes();
    cwd.push(0);
    if cwd.len() as u64 > size {
        return Err(ERANGE);
    }
    write_user(p, buf, &cwd)?;
    Ok(cwd.len() as u64)
}

/// `chdir(path)`.
pub fn chdir(p: &Process, ptr: u64) -> LinuxResult {
    let parts = parts_of(p, AT_FDCWD, ptr)?;
    match resolve(&parts)? {
        Found::Root | Found::DevDir | Found::EtcDir => {}
        Found::Item(_, info) if info.is_dir => {}
        _ => return Err(ENOTDIR),
    }
    let linux = p.linux().ok_or(EBADF)?;
    *linux.cwd.lock() = path::join(&parts);
    Ok(0)
}

/// `mkdirat(dirfd, path, mode)` (and `mkdir`).
pub fn mkdirat(p: &Process, dirfd: u64, ptr: u64) -> LinuxResult {
    let parts = parts_of(p, dirfd, ptr)?;
    if resolve(&parts).is_ok() {
        return Err(EEXIST);
    }
    let (parent, name) = parent_of(&parts)?;
    let fs = &parent.volume.fs;
    if fs.read_only() {
        return Err(EROFS);
    }
    fs.create(parent.id, &name, true)
        .map_err(errno::from_error)?;
    Ok(0)
}

/// `unlinkat(dirfd, path, flags)` (and `unlink`, `rmdir`).
pub fn unlinkat(p: &Process, dirfd: u64, ptr: u64, flags: u64) -> LinuxResult {
    let parts = parts_of(p, dirfd, ptr)?;
    let is_dir = match resolve(&parts)? {
        Found::Item(_, info) => info.is_dir,
        Found::Root | Found::DevDir | Found::EtcDir => return Err(errno::EBUSY),
        Found::Device(_) | Found::Config(_) => return Err(errno::EACCES),
    };
    if flags & AT_REMOVEDIR != 0 && !is_dir {
        return Err(ENOTDIR);
    }
    if flags & AT_REMOVEDIR == 0 && is_dir {
        return Err(EISDIR);
    }
    let (parent, name) = parent_of(&parts)?;
    let fs = &parent.volume.fs;
    if fs.read_only() {
        return Err(EROFS);
    }
    fs.remove(parent.id, &name).map_err(|e| match e {
        abi::Error::NotEmpty => ENOTEMPTY,
        e => errno::from_error(e),
    })?;
    Ok(0)
}

/// `renameat(olddirfd, old, newdirfd, new)` (and `rename`): within one
/// volume; an existing target file is replaced.
pub fn renameat(p: &Process, olddir: u64, old: u64, newdir: u64, new: u64) -> LinuxResult {
    let from = parts_of(p, olddir, old)?;
    let to = parts_of(p, newdir, new)?;
    let (from_parent, from_name) = parent_of(&from)?;
    let (to_parent, to_name) = parent_of(&to)?;
    if !Arc::ptr_eq(&from_parent.volume, &to_parent.volume) {
        return Err(EXDEV);
    }
    let fs = &from_parent.volume.fs;
    if fs.read_only() {
        return Err(EROFS);
    }
    if let Ok(existing) = fs.lookup(to_parent.id, &to_name) {
        if existing.is_dir {
            return Err(EISDIR);
        }
        if from == to {
            return Ok(0);
        }
        fs.remove(to_parent.id, &to_name)
            .map_err(errno::from_error)?;
    }
    fs.rename(from_parent.id, &from_name, to_parent.id, &to_name)
        .map_err(errno::from_error)?;
    Ok(0)
}

/// `d_type` values.
const DT_CHR: u8 = 2;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

/// The entries of a folder for `getdents64`: (inode, type, name), with
/// `.` and `..` first.
fn listing(node: &Option<Node>, path: &str) -> Result<Vec<(u64, u8, String)>, i64> {
    let mut entries = vec![
        (1, DT_DIR, String::from(".")),
        (1, DT_DIR, String::from("..")),
    ];
    match node {
        Some(node) => {
            for info in node.volume.fs.list(node.id).map_err(errno::from_error)? {
                let kind = if info.is_dir { DT_DIR } else { DT_REG };
                entries.push((info.id, kind, info.name));
            }
        }
        None if path == "/dev" => {
            for (i, name) in ["null", "zero", "tty"].iter().enumerate() {
                entries.push((3 + i as u64, DT_CHR, String::from(*name)));
            }
        }
        None if path == "/etc" => entries.push((9, DT_REG, String::from("resolv.conf"))),
        None => {
            for v in vfs::volumes() {
                let name = if v.name == "Tmp" {
                    String::from("tmp")
                } else {
                    v.name.clone()
                };
                entries.push((100 + u64::from(v.index), DT_DIR, name));
            }
            entries.push((2, DT_DIR, String::from("dev")));
            entries.push((8, DT_DIR, String::from("etc")));
        }
    }
    Ok(entries)
}

/// `getdents64(fd, buf, count)`: `struct linux_dirent64` records (inode
/// u64, offset i64, record length u16, type u8, name, NUL; 8-byte
/// aligned).
pub fn getdents64(p: &Process, fd: u64, buf: u64, count: u64) -> LinuxResult {
    let file = file_of(p, fd)?;
    let OpenFile::Dir { node, path, cursor } = &*file else {
        return Err(ENOTDIR);
    };
    let entries = listing(node, path)?;
    let mut cursor = cursor.lock();
    let mut out = Vec::new();
    while let Some((ino, kind, name)) = entries.get(*cursor as usize) {
        let reclen = (19 + name.len() + 1).next_multiple_of(8);
        if out.len() + reclen > count as usize {
            break;
        }
        let start = out.len();
        out.resize(start + reclen, 0);
        let rec = &mut out[start..];
        rec[0..8].copy_from_slice(&ino.to_le_bytes());
        rec[8..16].copy_from_slice(&(*cursor + 1).to_le_bytes());
        rec[16..18].copy_from_slice(&(reclen as u16).to_le_bytes());
        rec[18] = *kind;
        rec[19..19 + name.len()].copy_from_slice(name.as_bytes());
        *cursor += 1;
    }
    if out.is_empty() && (*cursor as usize) < entries.len() {
        // Not even one record fits.
        return Err(EINVAL);
    }
    write_user(p, buf, &out)?;
    Ok(out.len() as u64)
}
