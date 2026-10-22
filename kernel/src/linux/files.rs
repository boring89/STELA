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
use super::path::{self, Config, Device, PathError, Proc, Target};
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
pub(super) fn user_string(p: &Process, ptr: u64) -> Result<String, i64> {
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
    /// `/dev/pts`.
    PtsDir,
    /// Under `/proc`.
    Proc(Proc),
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

/// Whether the root shows volume `name` (a confined program's shows none,
/// P13 K15).
fn visible(p: &Process, name: &str) -> bool {
    p.linux()
        .is_some_and(|l| l.confined.is_none() && l.volumes.iter().any(|v| v == name))
}

/// Whether `rest` on volume `name` is where this process may go: for a
/// confined program (P13 K15), its folders and what is below them.
fn reachable(p: &Process, name: &str, rest: &[String]) -> bool {
    let Some(linux) = p.linux() else {
        return false;
    };
    match &linux.confined {
        None => name == "Tmp" || visible(p, name),
        Some(allowed) => {
            let mut parts = alloc::vec![String::from(name)];
            parts.extend_from_slice(rest);
            name == "Tmp" || path::within(&parts, allowed)
        }
    }
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

fn resolve(p: &Process, parts: &[String]) -> Result<Found, i64> {
    match path::target(parts) {
        Target::Root => Ok(Found::Root),
        Target::DevDir => Ok(Found::DevDir),
        Target::Device(Device::Pts(n)) if super::pty::find(n).is_none() => Err(ENOENT),
        Target::Device(d) => Ok(Found::Device(d)),
        Target::EtcDir => Ok(Found::EtcDir),
        Target::Config(c) => Ok(Found::Config(c)),
        Target::PtsDir => Ok(Found::PtsDir),
        Target::Missing => Err(ENOENT),
        Target::Proc(proc) => {
            if let Some(pid) = proc_pid(proc)
                && pid != p.id()
                && crate::task::process::find(pid).is_none()
            {
                return Err(ENOENT);
            }
            Ok(Found::Proc(proc))
        }
        Target::Lib(rest) => {
            let (node, info) = lib_node(&rest)?;
            Ok(Found::Item(node, info))
        }
        Target::Volume(name, rest) => {
            if !reachable(p, &name, &rest) {
                return Err(ENOENT);
            }
            let volume = vfs::find(&name).ok_or(ENOENT)?;
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

/// `rest` under the first library folder that has it (`/lib`, P13 K5;
/// every program sees them).
fn lib_node(rest: &[String]) -> Result<(Node, NodeInfo), i64> {
    for (volume, top, lib) in path::LIB_FOLDERS {
        let Some(volume) = vfs::find(volume) else {
            continue;
        };
        let mut parts = alloc::vec![String::from(top), String::from(lib)];
        parts.extend_from_slice(rest);
        if let Ok(info) = walk(&volume, &parts) {
            return Ok((
                Node {
                    volume,
                    id: info.id,
                },
                info,
            ));
        }
    }
    Err(ENOENT)
}

/// The bytes of a program interpreter or library named by an absolute
/// path under `/lib` or `/usr/lib` (P13 K5; `None` otherwise).
pub fn lib_image(path: &str) -> Option<Vec<u8>> {
    let parts = path::normalize("/", path).ok()?;
    let Target::Lib(rest) = path::target(&parts) else {
        return None;
    };
    let (node, info) = lib_node(&rest).ok()?;
    if info.is_dir {
        return None;
    }
    crate::program::read_program(&node.volume, node.id)
        .ok()
        .map(|(_, image)| image)
}

/// Whether file `node` may be mapped executable (P13 K5, `docs/linux.md`
/// §16): it is in a library folder (`System:/linux/lib`, `Initrd:/linux/lib`,
/// `Data:/Programs/lib`, `Data:/Programs/NAME/lib`) or in the program's
/// own folder.
pub(super) fn may_execute(p: &Process, node: &Node) -> bool {
    let Ok(path) = vfs::path_of(&node.volume, node.id) else {
        return false;
    };
    let folder = path.rsplit_once('/').map_or("", |(f, _)| f);
    let volume = node.volume.name.as_str();
    let in_lib = match volume {
        "Initrd" | "System" => folder == "/linux/lib" || folder.starts_with("/linux/lib/"),
        "Data" => {
            folder == "/Programs/lib"
                || folder.starts_with("/Programs/lib/")
                || folder
                    .strip_prefix("/Programs/")
                    .and_then(|r| r.split_once('/'))
                    .is_some_and(|(_, sub)| sub == "lib" || sub.starts_with("lib/"))
        }
        _ => false,
    };
    if in_lib {
        return true;
    }
    // The program's own folder (its /proc/self/exe without the name).
    let Some(linux) = p.linux() else {
        return false;
    };
    let exe = linux.exe.lock().clone();
    let own = String::from(exe.rsplit_once('/').map_or("", |(f, _)| f));
    let top = if volume == "Tmp" { "tmp" } else { volume };
    let here = alloc::format!("/{top}{folder}");
    !own.is_empty() && own == here
}

/// The parent folder (on a volume) and the last name of `parts`.
fn parent_of(p: &Process, parts: &[String]) -> Result<(Node, String), i64> {
    let (parent, name) = path::split_last(parts).ok_or(EEXIST)?;
    match resolve(p, parent)? {
        Found::Item(node, info) if info.is_dir => Ok((node, String::from(name))),
        Found::Item(..) => Err(ENOTDIR),
        // New names at the root or in /dev cannot be made.
        _ => Err(errno::EACCES),
    }
}

/// The file `path` names (relative to `dirfd`), for `execve`: its node
/// and its normalised Linux path. Folders and synthetic files → `EACCES`.
pub(super) fn exec_file(p: &Process, dirfd: u64, path: &str) -> Result<(Node, String), i64> {
    let base = base_of(p, dirfd)?;
    let parts = path::normalize(&base, path).map_err(|e| match e {
        PathError::Empty => ENOENT,
        PathError::TooLong => ENAMETOOLONG,
    })?;
    match resolve(p, &parts)? {
        Found::Item(node, info) if !info.is_dir => Ok((node, path::join(&parts))),
        _ => Err(errno::EACCES),
    }
}

/// The file an open descriptor refers to (`execveat` with
/// `AT_EMPTY_PATH`).
pub(super) fn fd_node(p: &Process, fd: u64) -> Result<Node, i64> {
    match &*file_of(p, fd)? {
        OpenFile::File { node, .. } => Ok(node.clone()),
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
    let found = match resolve(p, &parts) {
        Ok(found) => {
            if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                return Err(EEXIST);
            }
            found
        }
        Err(ENOENT) if flags & O_CREAT != 0 => {
            let (parent, name) = parent_of(p, &parts)?;
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
        Found::Root | Found::DevDir | Found::EtcDir | Found::PtsDir if writing => {
            return Err(EISDIR);
        }
        Found::Root | Found::DevDir | Found::EtcDir | Found::PtsDir => dir(None),
        Found::Proc(proc) if proc.is_dir() => dir(None),
        Found::Proc(_) if writing => return Err(errno::EACCES),
        Found::Proc(proc) => OpenFile::Text {
            data: proc_text(p, proc)?,
            offset: SpinLock::new(0),
        },
        Found::Device(Device::Random) => OpenFile::Random,
        Found::Device(Device::Ptmx) => {
            let (n, tty) = super::pty::create();
            OpenFile::PtyMaster(
                tty,
                n,
                core::sync::atomic::AtomicBool::new(flags & 0o4000 != 0),
            )
        }
        Found::Device(Device::Pts(n)) => {
            let tty = super::pty::find(n).ok_or(ENOENT)?;
            tty.add_user(p.id());
            OpenFile::Tty(tty)
        }
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

/// `pipe(fds)` and `pipe2(fds, flags)`: `O_CLOEXEC` and `O_NONBLOCK`.
pub fn pipe2(p: &Process, fds_ptr: u64, flags: u64) -> LinuxResult {
    const O_NONBLOCK: u64 = 0o4000;
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    if !p.space().check(fds_ptr, 8, true) {
        return Err(errno::EFAULT);
    }
    let (r, w) = crate::object::pipe::Pipe::create();
    let nb = flags & O_NONBLOCK != 0;
    let cloexec = flags & O_CLOEXEC != 0;
    use core::sync::atomic::AtomicBool;
    let rfd = add_fd(p, OpenFile::PipeRead(r, AtomicBool::new(nb)), cloexec)?;
    let wfd = match add_fd(p, OpenFile::PipeWrite(w, AtomicBool::new(nb)), cloexec) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = close(p, rfd);
            return Err(e);
        }
    };
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&(rfd as u32).to_le_bytes());
    b[4..].copy_from_slice(&(wfd as u32).to_le_bytes());
    write_user(p, fds_ptr, &b)?;
    Ok(0)
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
            const O_NONBLOCK: u64 = 0o4000;
            match &*file {
                OpenFile::File { state, .. } => {
                    let mut s = state.lock();
                    s.append = arg & O_APPEND != 0;
                    s.flags = (s.flags & !O_APPEND) | (arg & O_APPEND);
                }
                OpenFile::PipeRead(_, nb)
                | OpenFile::PipeWrite(_, nb)
                | OpenFile::PtyMaster(_, _, nb) => {
                    nb.store(arg & O_NONBLOCK != 0, core::sync::atomic::Ordering::Relaxed);
                }
                OpenFile::EventFd(e) => e.set_nonblock(arg & O_NONBLOCK != 0),
                OpenFile::TimerFd(t, _) => t.set_nonblock(arg & O_NONBLOCK != 0),
                _ => {}
            }
            Ok(0)
        }
        // Record locks (F_GETLK, F_SETLK, F_SETLKW and the OFD ones): one
        // writer at a time is all STELA offers, so every lock is granted
        // and F_GETLK finds none (P12).
        5 | 36 => {
            drop(fds);
            // struct flock: l_type (i16) = F_UNLCK (2).
            write_user(p, arg, &2u16.to_le_bytes())?;
            Ok(0)
        }
        6 | 7 | 37 | 38 => Ok(0),
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
        OpenFile::Random => {
            let mut buf = vec![0u8; len];
            frame::random::fill(&mut buf);
            Ok(buf)
        }
        OpenFile::PtyMaster(tty, _, nonblock) => {
            let deadline = nonblock
                .load(core::sync::atomic::Ordering::Relaxed)
                .then_some(0);
            tty.master_read(len, deadline).map_err(|e| match e {
                abi::Error::TimedOut => errno::EAGAIN,
                e => errno::from_error(e),
            })
        }
        OpenFile::Dir { .. } => Err(EISDIR),
        OpenFile::EventFd(e) => e.read(len),
        OpenFile::TimerFd(t, _) => t.read(len),
        OpenFile::Epoll(_) => Err(EINVAL),
        OpenFile::Socket(socket) => super::socket::read(socket, len),
        OpenFile::PipeRead(end, nonblock) => {
            let deadline = if nonblock.load(core::sync::atomic::Ordering::Relaxed) {
                Some(0)
            } else {
                None
            };
            end.0.read(len, deadline).map_err(|e| match e {
                abi::Error::TimedOut => errno::EAGAIN,
                e => errno::from_error(e),
            })
        }
        OpenFile::PipeWrite(..) => Err(EBADF),
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
        OpenFile::Null | OpenFile::Zero | OpenFile::Random => Ok(data.len()),
        OpenFile::PtyMaster(tty, ..) => {
            // Typed on the terminal: through its line discipline.
            for chunk in data.utf8_chunks() {
                tty.feed_str(chunk.valid());
            }
            Ok(data.len())
        }
        OpenFile::Dir { .. } => Err(EISDIR),
        OpenFile::EventFd(e) => e.write(data),
        OpenFile::TimerFd(..) | OpenFile::Epoll(_) => Err(EINVAL),
        OpenFile::Socket(socket) => super::socket::write(socket, data),
        OpenFile::Text { .. } | OpenFile::PipeRead(..) => Err(EBADF),
        OpenFile::PipeWrite(end, nonblock) => {
            let deadline = if nonblock.load(core::sync::atomic::Ordering::Relaxed) {
                Some(0)
            } else {
                None
            };
            let mut done = 0;
            while done < data.len() {
                match end.0.write(&data[done..], deadline) {
                    Ok(n) => done += n,
                    Err(abi::Error::TimedOut) if done > 0 => break,
                    Err(abi::Error::TimedOut) => return Err(errno::EAGAIN),
                    // EPIPE and SIGPIPE (P12; its default ends the
                    // process).
                    Err(abi::Error::PeerClosed) => {
                        super::signals::sigpipe();
                        return Err(errno::EPIPE);
                    }
                    Err(e) => return Err(errno::from_error(e)),
                }
            }
            Ok(done)
        }
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
const S_IFIFO: u32 = 0o010000;
const S_IFLNK: u32 = 0o120000;

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
        Found::Device(d) => stat_bytes(0, d.ino(), S_IFCHR | 0o666, 0, 0, 0),
        Found::PtsDir => stat_bytes(0, 13, S_IFDIR | 0o755, 0, 0, 0),
        Found::Proc(proc) if proc.is_dir() => stat_bytes(0, 14, S_IFDIR | 0o555, 0, 0, 0),
        Found::Proc(Proc::Exe(_) | Proc::Fd(..)) => stat_bytes(0, 15, S_IFLNK | 0o777, 0, 0, 0),
        Found::Proc(_) => stat_bytes(0, 16, S_IFREG | 0o444, 0, 0, 0),
        Found::Item(node, info) => {
            // P13 K8: STELA has no execute permission (any file can be
            // started), so files say 0755: programs that look for an
            // executable on PATH by its mode (git, which) find them.
            let mode = if info.is_dir {
                S_IFDIR | 0o755
            } else {
                S_IFREG | 0o755
            };
            let dev = u64::from(node.volume.index) + 1;
            stat_bytes(dev, info.id, mode, info.size, info.ctime, info.mtime)
        }
    }
}

/// `newfstatat(dirfd, path, buf, flags)` (and `stat`, `lstat`).
pub fn stat_path(p: &Process, dirfd: u64, ptr: u64, buf: u64, flags: u64) -> LinuxResult {
    let bytes = stat_at(p, dirfd, ptr, flags)?;
    write_user(p, buf, &bytes)?;
    Ok(0)
}

/// The `struct stat` of `path` relative to `dirfd` (or of `dirfd` itself
/// with `AT_EMPTY_PATH` and an empty path).
fn stat_at(p: &Process, dirfd: u64, ptr: u64, flags: u64) -> Result<[u8; 144], i64> {
    const AT_EMPTY_PATH: u64 = 0x1000;
    if flags & AT_EMPTY_PATH != 0 && user_string(p, ptr)?.is_empty() {
        return fstat_bytes(p, dirfd);
    }
    let parts = parts_of(p, dirfd, ptr)?;
    let found = resolve(p, &parts)?;
    Ok(stat_of(&found))
}

/// `statx(dirfd, path, flags, mask, buf)` (P13 K9): the `struct stat`
/// fields in `struct statx`'s layout; every basic field is filled in.
pub fn statx(p: &Process, dirfd: u64, ptr: u64, flags: u64, buf: u64) -> LinuxResult {
    let st = stat_at(p, dirfd, ptr, flags)?;
    let u64_at = |at: usize| u64::from_le_bytes(st[at..at + 8].try_into().unwrap_or([0; 8]));
    let u32_at = |at: usize| u32::from_le_bytes(st[at..at + 4].try_into().unwrap_or([0; 4]));
    let mut x = [0u8; 256];
    const STATX_BASIC_STATS: u32 = 0x7FF;
    x[0..4].copy_from_slice(&STATX_BASIC_STATS.to_le_bytes()); // stx_mask
    x[4..8].copy_from_slice(&4096u32.to_le_bytes()); // stx_blksize
    x[16..20].copy_from_slice(&(u64_at(16) as u32).to_le_bytes()); // stx_nlink
    x[28..30].copy_from_slice(&(u32_at(24) as u16).to_le_bytes()); // stx_mode
    x[32..40].copy_from_slice(&u64_at(8).to_le_bytes()); // stx_ino
    x[40..48].copy_from_slice(&u64_at(48).to_le_bytes()); // stx_size
    x[48..56].copy_from_slice(&u64_at(64).to_le_bytes()); // stx_blocks
    // stx_atime @64, stx_btime @80, stx_ctime @96, stx_mtime @112:
    // { i64 sec; u32 nsec; i32 pad }.
    for (dst, src) in [(64, 72), (80, 104), (96, 104), (112, 88)] {
        x[dst..dst + 8].copy_from_slice(&u64_at(src).to_le_bytes());
        x[dst + 8..dst + 12].copy_from_slice(&(u64_at(src + 8) as u32).to_le_bytes());
    }
    // stx_dev_major/minor @136/140: the device number's low byte.
    x[140..144].copy_from_slice(&(u64_at(0) as u32).to_le_bytes());
    write_user(p, buf, &x)?;
    Ok(0)
}

/// `fstat(fd, buf)`.
pub fn fstat(p: &Process, fd: u64, buf: u64) -> LinuxResult {
    let bytes = fstat_bytes(p, fd)?;
    write_user(p, buf, &bytes)?;
    Ok(0)
}

/// The `struct stat` of an open descriptor.
fn fstat_bytes(p: &Process, fd: u64) -> Result<[u8; 144], i64> {
    let file = file_of(p, fd)?;
    let bytes = match &*file {
        OpenFile::Tty(_) => stat_bytes(0, 6, S_IFCHR | 0o620, 0, 0, 0),
        OpenFile::Null => stat_bytes(0, 3, S_IFCHR | 0o666, 0, 0, 0),
        OpenFile::Zero => stat_bytes(0, 4, S_IFCHR | 0o666, 0, 0, 0),
        OpenFile::Random => stat_bytes(0, 11, S_IFCHR | 0o666, 0, 0, 0),
        OpenFile::PtyMaster(_, n, _) => {
            stat_bytes(0, 0x2000 + u64::from(*n), S_IFCHR | 0o620, 0, 0, 0)
        }
        OpenFile::Socket(_) => stat_bytes(0, 7, S_IFSOCK | 0o777, 0, 0, 0),
        // anon_inode files (P13 K6).
        OpenFile::EventFd(_) | OpenFile::TimerFd(..) | OpenFile::Epoll(_) => {
            stat_bytes(0, 13, 0o600, 0, 0, 0)
        }
        OpenFile::PipeRead(..) | OpenFile::PipeWrite(..) => {
            stat_bytes(0, 10, S_IFIFO | 0o600, 0, 0, 0)
        }
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
    Ok(bytes)
}

/// The volume a path or descriptor is on (P13 K9, statfs): `None` for
/// the synthetic places (/, /dev, /proc...).
fn volume_of_found(found: &Found) -> Option<Arc<Volume>> {
    match found {
        Found::Item(node, _) => Some(Arc::clone(&node.volume)),
        _ => None,
    }
}

/// A `struct statfs` (120 bytes) for `volume` (or a memory file system
/// with nothing in it).
fn statfs_bytes(volume: Option<&Volume>) -> [u8; 120] {
    let (total, free, read_only) = volume.map_or((0, 0, false), |v| {
        let (t, f) = v.fs.space();
        (t, f, v.fs.read_only())
    });
    let mut b = [0u8; 120];
    let put =
        |b: &mut [u8; 120], at: usize, v: u64| b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    // f_type: "STLA"; f_bsize; f_blocks; f_bfree; f_bavail; f_files;
    // f_ffree; f_fsid; f_namelen; f_frsize; f_flags (ST_RDONLY = 1).
    put(&mut b, 0, 0x5354_4C41);
    put(&mut b, 8, 4096);
    put(&mut b, 16, total / 4096);
    put(&mut b, 24, free / 4096);
    put(&mut b, 32, free / 4096);
    put(&mut b, 40, 1 << 20);
    put(&mut b, 48, 1 << 19);
    put(&mut b, 64, 255);
    put(&mut b, 72, 4096);
    put(&mut b, 80, u64::from(read_only));
    b
}

/// `statfs(path, buf)` (P13 K9).
pub fn statfs(p: &Process, ptr: u64, buf: u64) -> LinuxResult {
    let parts = parts_of(p, AT_FDCWD, ptr)?;
    let found = resolve(p, &parts)?;
    write_user(p, buf, &statfs_bytes(volume_of_found(&found).as_deref()))?;
    Ok(0)
}

/// `fstatfs(fd, buf)` (P13 K9).
pub fn fstatfs(p: &Process, fd: u64, buf: u64) -> LinuxResult {
    let file = file_of(p, fd)?;
    let volume = match &*file {
        OpenFile::File { node, .. }
        | OpenFile::Dir {
            node: Some(node), ..
        } => Some(Arc::clone(&node.volume)),
        _ => None,
    };
    write_user(p, buf, &statfs_bytes(volume.as_deref()))?;
    Ok(0)
}

/// `sendfile(out, in, offset_ptr, count)` and `copy_file_range(in,
/// off_in, out, off_out, len, flags)` (P13 K9): a plain copy through the
/// kernel. With an offset pointer the input is read there (and the pointer
/// moved) without moving the descriptor's offset.
pub fn copy_between(p: &Process, out_fd: u64, in_fd: u64, off_ptr: u64, count: u64) -> LinuxResult {
    let input = file_of(p, in_fd)?;
    let output = file_of(p, out_fd)?;
    let mut offset = if off_ptr != 0 {
        Some(super::sys::read_u64(p, off_ptr)?)
    } else {
        None
    };
    let mut done = 0u64;
    while done < count {
        let want = ((count - done) as usize).min(CHUNK);
        let data = match (offset, &*input) {
            (Some(at), OpenFile::File { node, .. }) => {
                let mut buf = vec![0u8; want];
                let n = node
                    .volume
                    .fs
                    .read(node.id, at, &mut buf)
                    .map_err(errno::from_error)?;
                buf.truncate(n);
                buf
            }
            (Some(_), _) => return Err(errno::ESPIPE),
            (None, file) => read_file(file, want)?,
        };
        if data.is_empty() {
            break;
        }
        let n = write_file(&output, &data)?;
        done += n as u64;
        if let Some(at) = offset.as_mut() {
            *at += n as u64;
        }
        if n < data.len() {
            break;
        }
    }
    if let Some(at) = offset {
        write_user(p, off_ptr, &at.to_le_bytes())?;
    }
    Ok(done)
}

/// How many `memfd_create` files there were (their names).
static MEMFDS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `memfd_create(name, flags)` (P13 K9): a file on `Tmp` (memory), named
/// `.memfd-PID-N`; it stays there until `Tmp` is emptied (restart).
pub fn memfd_create(p: &Process, ptr: u64, flags: u64) -> LinuxResult {
    const MFD_CLOEXEC: u64 = 1;
    let _ = user_string(p, ptr)?;
    let tmp = vfs::find("Tmp").ok_or(ENOENT)?;
    let n = MEMFDS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let name = alloc::format!(".memfd-{}-{n}", p.id());
    let info = tmp
        .fs
        .create(tmp.fs.root(), &name, false)
        .map_err(errno::from_error)?;
    let file = OpenFile::File {
        node: Node {
            volume: tmp,
            id: info.id,
        },
        state: SpinLock::new(FileState {
            offset: 0,
            append: false,
            readable: true,
            writable: true,
            flags: 2,
        }),
    };
    add_fd(p, file, flags & MFD_CLOEXEC != 0)
}

/// `faccessat(dirfd, path, mode)` (and `access`): it exists, and is
/// writable unless on a read-only volume.
pub fn access(p: &Process, dirfd: u64, ptr: u64, mode: u64) -> LinuxResult {
    const W_OK: u64 = 2;
    let parts = parts_of(p, dirfd, ptr)?;
    let found = resolve(p, &parts)?;
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
    match resolve(p, &parts)? {
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
    if resolve(p, &parts).is_ok() {
        return Err(EEXIST);
    }
    let (parent, name) = parent_of(p, &parts)?;
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
    let is_dir = match resolve(p, &parts)? {
        Found::Item(_, info) => info.is_dir,
        Found::Root | Found::DevDir | Found::EtcDir | Found::PtsDir => {
            return Err(errno::EBUSY);
        }
        Found::Device(_) | Found::Config(_) | Found::Proc(_) => return Err(errno::EACCES),
    };
    if flags & AT_REMOVEDIR != 0 && !is_dir {
        return Err(ENOTDIR);
    }
    if flags & AT_REMOVEDIR == 0 && is_dir {
        return Err(EISDIR);
    }
    let (parent, name) = parent_of(p, &parts)?;
    let fs = &parent.volume.fs;
    if fs.read_only() {
        return Err(EROFS);
    }
    fs.check_unlocked(parent.id, &name)
        .map_err(errno::from_error)?;
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
    let (from_parent, from_name) = parent_of(p, &from)?;
    let (to_parent, to_name) = parent_of(p, &to)?;
    if !Arc::ptr_eq(&from_parent.volume, &to_parent.volume) {
        return Err(EXDEV);
    }
    let fs = &from_parent.volume.fs;
    if fs.read_only() {
        return Err(EROFS);
    }
    fs.check_unlocked(from_parent.id, &from_name)
        .map_err(errno::from_error)?;
    if let Ok(existing) = fs.lookup(to_parent.id, &to_name) {
        fs.check_unlocked(to_parent.id, &to_name)
            .map_err(errno::from_error)?;
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
const DT_LNK: u8 = 10;

/// The entries of a folder for `getdents64`: (inode, type, name), with
/// `.` and `..` first.
fn listing(p: &Process, node: &Option<Node>, path: &str) -> Result<Vec<(u64, u8, String)>, i64> {
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
            for (ino, name) in [
                (3, "null"),
                (4, "zero"),
                (5, "tty"),
                (11, "urandom"),
                (11, "random"),
                (12, "ptmx"),
            ] {
                entries.push((ino, DT_CHR, String::from(name)));
            }
            entries.push((13, DT_DIR, String::from("pts")));
        }
        None if path == "/dev/pts" => {
            for n in super::pty::numbers() {
                entries.push((0x1000 + u64::from(n), DT_CHR, alloc::format!("{n}")));
            }
        }
        None if path == "/proc" => {
            entries.push((14, DT_DIR, String::from("self")));
            for name in ["cpuinfo", "meminfo", "uptime"] {
                entries.push((16, DT_REG, String::from(name)));
            }
            for (id, _, _, code) in crate::task::process::list() {
                if code.is_none() {
                    entries.push((0x10_0000 + id, DT_DIR, alloc::format!("{id}")));
                }
            }
        }
        None if path.starts_with("/proc/") && path.ends_with("/fd") => {
            if let Some(linux) = p.linux() {
                let numbers = linux.fds.lock().numbers();
                for fd in numbers {
                    entries.push((0x20_0000 + fd, DT_LNK, alloc::format!("{fd}")));
                }
            }
        }
        None if path.starts_with("/proc/") => {
            for (name, kind) in [
                ("exe", DT_LNK),
                ("maps", DT_REG),
                ("stat", DT_REG),
                ("status", DT_REG),
                ("cmdline", DT_REG),
                ("fd", DT_DIR),
            ] {
                entries.push((17, kind, String::from(name)));
            }
        }
        None if path == "/etc" => entries.push((9, DT_REG, String::from("resolv.conf"))),
        None => {
            for v in vfs::volumes()
                .into_iter()
                .filter(|v| v.name == "Tmp" || visible(p, &v.name))
            {
                let name = if v.name == "Tmp" {
                    String::from("tmp")
                } else {
                    v.name.clone()
                };
                entries.push((100 + u64::from(v.index), DT_DIR, name));
            }
            entries.push((2, DT_DIR, String::from("dev")));
            entries.push((8, DT_DIR, String::from("etc")));
            entries.push((14, DT_DIR, String::from("proc")));
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
    let entries = listing(p, node, path)?;
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

/// The process a `/proc` entry is about (`None`: the caller).
fn proc_pid(proc: Proc) -> Option<u64> {
    match proc {
        Proc::PidDir(pid)
        | Proc::Exe(pid)
        | Proc::Maps(pid)
        | Proc::Stat(pid)
        | Proc::Status(pid)
        | Proc::Cmdline(pid)
        | Proc::FdDir(pid)
        | Proc::Fd(pid, _) => pid,
        Proc::Dir | Proc::Cpuinfo | Proc::Meminfo | Proc::Uptime => None,
    }
}

/// What `/proc/N/stat` and `status` report about process `pid`.
fn proc_info(p: &Process, pid: u64) -> Result<super::procfs::ProcInfo, i64> {
    let process = crate::task::process::find(pid).ok_or(ENOENT)?;
    let linux = process.linux();
    let load = core::sync::atomic::Ordering::Relaxed;
    Ok(super::procfs::ProcInfo {
        pid,
        name: String::from(process.name()),
        state: if process.exiting() {
            'Z'
        } else if pid == p.id() {
            'R'
        } else {
            'S'
        },
        ppid: linux.map_or(0, |l| l.parent.load(load)),
        pgid: linux.map_or(pid, |l| l.pgid.load(load)),
        sid: linux.map_or(pid, |l| l.sid.load(load)),
        threads: process.live_threads() as u64,
        pages: 0,
    })
}

/// The bytes of a `/proc` file.
fn proc_text(p: &Process, proc: Proc) -> Result<Vec<u8>, i64> {
    let me = p.id();
    let pid = proc_pid(proc).unwrap_or(me);
    let text = match proc {
        Proc::Cpuinfo => {
            let info = frame::cpuid::info();
            let mhz = frame::time::tsc_hz() / 1_000_000;
            super::procfs::cpuinfo(frame::cpu::online(), &info.vendor, &info.brand, mhz)
        }
        Proc::Meminfo => {
            let (free, total) = frame::mm::pmm::stats();
            super::procfs::meminfo(total as u64 * 4096, free as u64 * 4096)
        }
        Proc::Uptime => super::procfs::uptime(frame::time::now_ns()),
        Proc::Stat(_) => super::procfs::stat(&proc_info(p, pid)?),
        Proc::Status(_) => super::procfs::status(&proc_info(p, pid)?),
        Proc::Cmdline(_) => {
            let mut b = proc_info(p, pid)?.name;
            b.push('\0');
            b
        }
        Proc::Maps(_) if pid == me => maps_text(p)?,
        Proc::Exe(_) | Proc::Fd(..) => return Err(errno::EINVAL),
        _ => return Err(ENOENT),
    };
    Ok(text.into_bytes())
}

/// `/proc/self/maps`: the program, the break area, the mappings, the
/// stack.
fn maps_text(p: &Process) -> Result<String, i64> {
    let linux = p.linux().ok_or(ENOENT)?;
    let exe = linux.exe.lock().clone();
    let (start, mapped) = {
        let m = linux.memory.lock();
        (m.start, m.mapped)
    };
    let mut out = super::procfs::maps_line(0x40_0000, start, false, true, &exe);
    if mapped > start {
        out.push_str(&super::procfs::maps_line(
            start, mapped, true, false, "[heap]",
        ));
    }
    for (at, size) in p.maps().lock().ranges() {
        out.push_str(&super::procfs::maps_line(at, at + size, true, false, ""));
    }
    let top = abi::layout::STACK_TOP;
    out.push_str(&super::procfs::maps_line(
        top - abi::layout::STACK_SIZE,
        top,
        true,
        false,
        "[stack]",
    ));
    Ok(out)
}

/// What `/proc/self/fd/N` points at.
fn fd_link(p: &Process, fd: u64) -> Result<String, i64> {
    let file = file_of(p, fd).map_err(|_| ENOENT)?;
    Ok(match &*file {
        OpenFile::Tty(_) => String::from("/dev/tty"),
        OpenFile::Null => String::from("/dev/null"),
        OpenFile::Zero => String::from("/dev/zero"),
        OpenFile::Random => String::from("/dev/urandom"),
        OpenFile::PtyMaster(..) => String::from("/dev/ptmx"),
        OpenFile::Dir { path, .. } => path.clone(),
        OpenFile::File { node, .. } => linux_path_of(node),
        OpenFile::Socket(_) => alloc::format!("socket:[{fd}]"),
        OpenFile::PipeRead(..) | OpenFile::PipeWrite(..) => alloc::format!("pipe:[{fd}]"),
        OpenFile::Text { .. } => String::from("/proc"),
        OpenFile::EventFd(_) => String::from("anon_inode:[eventfd]"),
        OpenFile::TimerFd(..) => String::from("anon_inode:[timerfd]"),
        OpenFile::Epoll(_) => String::from("anon_inode:[eventpoll]"),
    })
}

/// The Linux path of a file on a volume.
fn linux_path_of(node: &Node) -> String {
    let inner = vfs::path_of(&node.volume, node.id).unwrap_or_default();
    let top = if node.volume.name == "Tmp" {
        "tmp"
    } else {
        node.volume.name.as_str()
    };
    alloc::format!("/{top}/{}", inner.trim_start_matches('/'))
        .trim_end_matches('/')
        .into()
}

/// `readlinkat(dirfd, path, buf, size)` (and `readlink`): only `/proc`'s
/// links exist (`exe`, `fd/N`); other files are not links (`EINVAL`).
pub fn readlinkat(p: &Process, dirfd: u64, ptr: u64, buf: u64, size: u64) -> LinuxResult {
    let parts = parts_of(p, dirfd, ptr)?;
    let target = match resolve(p, &parts)? {
        Found::Proc(Proc::Exe(pid)) => {
            let pid = pid.unwrap_or(p.id());
            let process = if pid == p.id() {
                None
            } else {
                Some(crate::task::process::find(pid).ok_or(ENOENT)?)
            };
            let linux = match &process {
                Some(other) => other.linux(),
                None => p.linux(),
            }
            .ok_or(ENOENT)?;
            let exe = linux.exe.lock().clone();
            if exe.is_empty() {
                return Err(ENOENT);
            }
            exe
        }
        Found::Proc(Proc::Fd(None, fd)) => fd_link(p, fd)?,
        _ => return Err(EINVAL),
    };
    let n = target.len().min(size as usize);
    write_user(p, buf, &target.as_bytes()[..n])?;
    Ok(n as u64)
}

/// The path `/proc/self/exe` stands for, for `execve` (else the path).
pub(super) fn exec_alias(p: &Process, path: &str) -> String {
    if matches!(path, "/proc/self/exe" | "/proc/thread-self/exe")
        && let Some(linux) = p.linux()
    {
        let exe = linux.exe.lock().clone();
        if !exe.is_empty() {
            return exe;
        }
    }
    String::from(path)
}

/// `pread64(fd, buf, len, offset)`: a file read at `offset`, the file's
/// own offset unchanged.
pub fn pread(p: &Process, fd: u64, buf: u64, len: u64, offset: u64) -> LinuxResult {
    let file = file_of(p, fd)?;
    let OpenFile::File { node, state } = &*file else {
        return Err(ESPIPE);
    };
    if !state.lock().readable {
        return Err(EBADF);
    }
    let mut data = vec![0u8; len.min(CHUNK as u64) as usize];
    let n = node
        .volume
        .fs
        .read(node.id, offset, &mut data)
        .map_err(errno::from_error)?;
    write_user(p, buf, &data[..n])?;
    Ok(n as u64)
}

/// `pwrite64(fd, buf, len, offset)`.
pub fn pwrite(p: &Process, fd: u64, buf: u64, len: u64, offset: u64) -> LinuxResult {
    let file = file_of(p, fd)?;
    let OpenFile::File { node, state } = &*file else {
        return Err(ESPIPE);
    };
    if !state.lock().writable {
        return Err(EBADF);
    }
    let data = read_user(p, buf, len.min(CHUNK as u64) as usize)?;
    node.volume
        .fs
        .write(node.id, offset, &data)
        .map_err(errno::from_error)?;
    Ok(data.len() as u64)
}

/// `ftruncate(fd, length)`.
pub fn ftruncate(p: &Process, fd: u64, length: u64) -> LinuxResult {
    let file = file_of(p, fd)?;
    let OpenFile::File { node, state } = &*file else {
        return Err(EINVAL);
    };
    if !state.lock().writable {
        return Err(EINVAL);
    }
    node.volume
        .fs
        .set_size(node.id, length)
        .map_err(errno::from_error)?;
    Ok(0)
}

/// `truncate(path, length)`.
pub fn truncate(p: &Process, ptr: u64, length: u64) -> LinuxResult {
    let parts = parts_of(p, AT_FDCWD, ptr)?;
    match resolve(p, &parts)? {
        Found::Item(_, info) if info.is_dir => Err(EISDIR),
        Found::Item(node, _) => {
            if node.volume.fs.read_only() {
                return Err(EROFS);
            }
            node.volume
                .fs
                .set_size(node.id, length)
                .map_err(errno::from_error)?;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

/// `fchdir(fd)`.
pub fn fchdir(p: &Process, fd: u64) -> LinuxResult {
    let linux = p.linux().ok_or(EBADF)?;
    match &*file_of(p, fd)? {
        OpenFile::Dir { path, .. } => {
            *linux.cwd.lock() = path.clone();
            Ok(0)
        }
        _ => Err(ENOTDIR),
    }
}

/// `dup3(fd, to, flags)`.
pub fn dup3(p: &Process, fd: u64, to: u64, flags: u64) -> LinuxResult {
    if fd == to || flags & !O_CLOEXEC != 0 {
        return Err(EINVAL);
    }
    let linux = p.linux().ok_or(EBADF)?;
    let (new, old) = {
        let mut fds = linux.fds.lock();
        let r = fds.dup2(fd, to).map_err(fd_error)?;
        if let Ok(slot) = fds.get_mut(to) {
            slot.cloexec = flags & O_CLOEXEC != 0;
        }
        r
    };
    drop(old);
    Ok(new as u64)
}

/// Calls that only need the path to exist (`chmod`, `chown`, `utimensat`:
/// STELA keeps no modes, owners or settable times).
pub fn exists(p: &Process, dirfd: u64, ptr: u64) -> LinuxResult {
    if ptr == 0 {
        // utimensat(fd, NULL, ...): the descriptor itself.
        file_of(p, dirfd)?;
        return Ok(0);
    }
    let parts = parts_of(p, dirfd, ptr)?;
    resolve(p, &parts)?;
    Ok(0)
}

/// Bytes readable at once (`FIONREAD`).
pub fn readable_bytes(file: &OpenFile) -> u64 {
    match file {
        OpenFile::Tty(tty) => u64::from(tty.readable()),
        OpenFile::PtyMaster(tty, ..) => u64::from(tty.master_readable()),
        OpenFile::PipeRead(end, _) => end.0.len() as u64,
        OpenFile::File { node, state } => {
            let at = state.lock().offset;
            node.volume
                .fs
                .get(node.id)
                .map_or(0, |i| i.size.saturating_sub(at))
        }
        _ => 0,
    }
}
