// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! BSD sockets for Linux programs (`docs/net.md`, `docs/linux.md`): only
//! `AF_INET` with `SOCK_STREAM` (TCP) or `SOCK_DGRAM` (UDP), mapped onto
//! the kernel's sockets (`crate::net`). A socket is a descriptor
//! (`OpenFile::Socket`); `read`/`write` on it are `recv`/`send`.
//!
//! `struct sockaddr_in` (`netinet/in.h`): family (u16, 2), port (big
//! endian u16), address (4 bytes), 8 bytes of zero.

use super::errno;
use super::errno::{EAFNOSUPPORT, EBADF, EINVAL, ENOTCONN, ENOTSOCK, EOPNOTSUPP, EPROTONOSUPPORT};
use super::files::add_fd;
use super::state::OpenFile;
use super::sys::{LinuxResult, file_of, read_user, write_user};
use crate::task::Process;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use net::Kind;

const AF_INET: u64 = 2;
const SOCK_STREAM: u64 = 1;
const SOCK_DGRAM: u64 = 2;
const SOCK_TYPE_MASK: u64 = 0xF;
const SOCK_NONBLOCK: u64 = 0o4000;
const SOCK_CLOEXEC: u64 = 0o2000000;
const IPPROTO_TCP: u64 = 6;
const IPPROTO_UDP: u64 = 17;
const MSG_DONTWAIT: u64 = 0x40;
const SHUT_RD: u64 = 0;

/// A Linux socket: the kernel socket and how it was made.
pub struct LinuxSocket {
    socket: Arc<crate::net::Socket>,
    kind: Kind,
    nonblock: AtomicBool,
}

impl LinuxSocket {
    /// The kernel socket (for `poll`).
    pub fn kernel_socket(&self) -> &Arc<crate::net::Socket> {
        &self.socket
    }
}

fn socket_of(p: &Process, fd: u64) -> Result<Arc<LinuxSocket>, i64> {
    match &*file_of(p, fd)? {
        OpenFile::Socket(s) => Ok(Arc::clone(s)),
        _ => Err(ENOTSOCK),
    }
}

/// Reads a `sockaddr_in` of the caller.
fn read_addr(p: &Process, ptr: u64, len: u64) -> Result<([u8; 4], u16), i64> {
    if len < 8 {
        return Err(EINVAL);
    }
    let b = read_user(p, ptr, 8)?;
    if u64::from(u16::from_le_bytes([b[0], b[1]])) != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    Ok(([b[4], b[5], b[6], b[7]], u16::from_be_bytes([b[2], b[3]])))
}

/// Writes a `sockaddr_in` to `ptr` and its length to `*len_ptr`
/// (cut to the caller's buffer, as Linux does).
fn write_addr(p: &Process, ptr: u64, len_ptr: u64, ip: [u8; 4], port: u16) -> Result<(), i64> {
    if ptr == 0 || len_ptr == 0 {
        return Ok(());
    }
    let room = u32::from_le_bytes(read_user(p, len_ptr, 4)?.try_into().map_err(|_| EINVAL)?);
    let mut sa = [0u8; 16];
    sa[0..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
    sa[2..4].copy_from_slice(&port.to_be_bytes());
    sa[4..8].copy_from_slice(&ip);
    write_user(p, ptr, &sa[..(room as usize).min(16)])?;
    write_user(p, len_ptr, &16u32.to_le_bytes())
}

fn timeout(s: &LinuxSocket, flags: u64) -> u64 {
    if s.nonblock.load(Ordering::Relaxed) || flags & MSG_DONTWAIT != 0 {
        0
    } else {
        u64::MAX
    }
}

fn nonblocking_error(e: abi::Error, nonblock: bool) -> i64 {
    if e == abi::Error::TimedOut && nonblock {
        errno::EAGAIN
    } else {
        errno::from_error(e)
    }
}

/// `socket(domain, type, protocol)`.
pub fn socket(p: &Process, domain: u64, ty: u64, protocol: u64) -> LinuxResult {
    if !p.linux().is_some_and(|l| l.network) {
        return Err(errno::EACCES);
    }
    if domain != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let kind = match (ty & SOCK_TYPE_MASK, protocol) {
        (SOCK_STREAM, 0 | IPPROTO_TCP) => Kind::Tcp,
        (SOCK_DGRAM, 0 | IPPROTO_UDP) => Kind::Udp,
        _ => return Err(EPROTONOSUPPORT),
    };
    let socket = crate::net::create(kind).map_err(errno::from_error)?;
    let file = OpenFile::Socket(Arc::new(LinuxSocket {
        socket,
        kind,
        nonblock: AtomicBool::new(ty & SOCK_NONBLOCK != 0),
    }));
    add_fd(p, file, ty & SOCK_CLOEXEC != 0)
}

/// `connect(fd, addr, len)`.
pub fn connect(p: &Process, fd: u64, addr: u64, len: u64) -> LinuxResult {
    let s = socket_of(p, fd)?;
    let (ip, port) = read_addr(p, addr, len)?;
    crate::net::connect(&s.socket, ip, port, u64::MAX)
        .map(|()| 0)
        .map_err(errno::from_error)
}

/// `bind(fd, addr, len)`: the port (the address must be ours or any).
pub fn bind(p: &Process, fd: u64, addr: u64, len: u64) -> LinuxResult {
    let s = socket_of(p, fd)?;
    let (_, port) = read_addr(p, addr, len)?;
    crate::net::bind(&s.socket, port)
        .map(|_| 0)
        .map_err(errno::from_error)
}

/// `listen(fd, backlog)`.
pub fn listen(p: &Process, fd: u64, backlog: u64) -> LinuxResult {
    let s = socket_of(p, fd)?;
    if s.kind != Kind::Tcp {
        return Err(EOPNOTSUPP);
    }
    crate::net::listen(&s.socket, (backlog as usize).max(1))
        .map(|()| 0)
        .map_err(errno::from_error)
}

/// `accept(fd, addr, len)` and `accept4(..., flags)`.
pub fn accept(p: &Process, fd: u64, addr: u64, len: u64, flags: u64) -> LinuxResult {
    let s = socket_of(p, fd)?;
    let nonblock = s.nonblock.load(Ordering::Relaxed);
    let conn = crate::net::accept(&s.socket, timeout(&s, 0))
        .map_err(|e| nonblocking_error(e, nonblock))?;
    let (ip, port) = crate::net::peer_addr(&conn).map_err(errno::from_error)?;
    write_addr(p, addr, len, ip, port)?;
    let file = OpenFile::Socket(Arc::new(LinuxSocket {
        socket: conn,
        kind: Kind::Tcp,
        nonblock: AtomicBool::new(flags & SOCK_NONBLOCK != 0),
    }));
    add_fd(p, file, flags & SOCK_CLOEXEC != 0)
}

/// `sendto(fd, buf, len, flags, addr, addr_len)`.
pub fn sendto(p: &Process, fd: u64, buf: u64, len: u64, addr: u64, alen: u64) -> LinuxResult {
    let s = socket_of(p, fd)?;
    let data = read_user(p, buf, (len as usize).min(crate::net::CHUNK))?;
    let to = if addr != 0 && s.kind == Kind::Udp {
        Some(read_addr(p, addr, alen)?)
    } else {
        None
    };
    crate::net::send(&s.socket, &data, to)
        .map(|n| n as u64)
        .map_err(|e| match e {
            abi::Error::InvalidArgument => ENOTCONN,
            e => errno::from_error(e),
        })
}

/// `recvfrom(fd, buf, len, flags, addr, addr_len)`; `MSG_DONTWAIT` does
/// not wait.
pub fn recvfrom(p: &Process, args: [u64; 6]) -> LinuxResult {
    let [fd, buf, len, flags, addr, alen] = args;
    let s = socket_of(p, fd)?;
    let nonblock = timeout(&s, flags) == 0;
    let (data, ip, port) = crate::net::recv(
        &s.socket,
        (len as usize).min(crate::net::CHUNK),
        timeout(&s, flags),
    )
    .map_err(|e| nonblocking_error(e, nonblock))?;
    write_user(p, buf, &data)?;
    write_addr(p, addr, alen, ip, port)?;
    Ok(data.len() as u64)
}

/// The (base, len) pairs of the caller's iovec array.
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

/// A `struct msghdr` (56 bytes): name, name length, iovecs and count;
/// control data is not supported (its length must be 0).
struct MsgHdr {
    name: u64,
    name_len: u64,
    iov: Vec<(u64, u64)>,
}

fn read_msghdr(p: &Process, ptr: u64) -> Result<MsgHdr, i64> {
    let b = read_user(p, ptr, 56)?;
    let w = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
    let (name, name_len, iov, iovlen, control_len) =
        (w(0), w(8) & 0xFFFF_FFFF, w(16), w(24), w(40));
    if control_len != 0 {
        return Err(EOPNOTSUPP);
    }
    Ok(MsgHdr {
        name,
        name_len,
        iov: iovecs(p, iov, iovlen)?,
    })
}

/// `sendmsg(fd, msg, flags)`: the iovecs as one datagram (or stream
/// piece) to `msg_name`.
pub fn sendmsg(p: &Process, fd: u64, msg: u64) -> LinuxResult {
    let s = socket_of(p, fd)?;
    let h = read_msghdr(p, msg)?;
    let mut data = Vec::new();
    for (base, len) in &h.iov {
        if data.len() + *len as usize > crate::net::CHUNK {
            return Err(errno::EMSGSIZE);
        }
        data.extend_from_slice(&read_user(p, *base, *len as usize)?);
    }
    let to = if h.name != 0 && s.kind == Kind::Udp {
        Some(read_addr(p, h.name, h.name_len)?)
    } else {
        None
    };
    crate::net::send(&s.socket, &data, to)
        .map(|n| n as u64)
        .map_err(|e| match e {
            abi::Error::InvalidArgument => ENOTCONN,
            e => errno::from_error(e),
        })
}

/// `recvmsg(fd, msg, flags)`: one receive spread over the iovecs; the
/// source goes to `msg_name`, `msg_namelen` and `msg_flags` are updated.
pub fn recvmsg(p: &Process, fd: u64, msg: u64, flags: u64) -> LinuxResult {
    let s = socket_of(p, fd)?;
    let h = read_msghdr(p, msg)?;
    let total: u64 = h.iov.iter().map(|v| v.1).sum();
    let nonblock = timeout(&s, flags) == 0;
    let (data, ip, port) = crate::net::recv(
        &s.socket,
        (total as usize).min(crate::net::CHUNK),
        timeout(&s, flags),
    )
    .map_err(|e| nonblocking_error(e, nonblock))?;
    let mut at = 0usize;
    for (base, len) in &h.iov {
        if at >= data.len() {
            break;
        }
        let n = (*len as usize).min(data.len() - at);
        write_user(p, *base, &data[at..at + n])?;
        at += n;
    }
    if h.name != 0 {
        let mut sa = [0u8; 16];
        sa[0..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
        sa[2..4].copy_from_slice(&port.to_be_bytes());
        sa[4..8].copy_from_slice(&ip);
        write_user(p, h.name, &sa[..(h.name_len as usize).min(16)])?;
        write_user(p, msg + 8, &16u32.to_le_bytes())?;
    }
    // msg_controllen and msg_flags.
    write_user(p, msg + 40, &0u64.to_le_bytes())?;
    write_user(p, msg + 48, &0u32.to_le_bytes())?;
    Ok(data.len() as u64)
}

/// `read` on a socket.
pub fn read(s: &LinuxSocket, len: usize) -> Result<Vec<u8>, i64> {
    let nonblock = timeout(s, 0) == 0;
    crate::net::recv(&s.socket, len, timeout(s, 0))
        .map(|(data, ..)| data)
        .map_err(|e| nonblocking_error(e, nonblock))
}

/// `write` on a socket.
pub fn write(s: &LinuxSocket, data: &[u8]) -> Result<usize, i64> {
    crate::net::send(&s.socket, data, None).map_err(|e| match e {
        abi::Error::InvalidArgument => ENOTCONN,
        e => errno::from_error(e),
    })
}

/// `shutdown(fd, how)`: `SHUT_WR` and `SHUT_RDWR` send FIN; `SHUT_RD` is
/// accepted and does nothing.
pub fn shutdown(p: &Process, fd: u64, how: u64) -> LinuxResult {
    let s = socket_of(p, fd)?;
    if how == SHUT_RD {
        return Ok(0);
    }
    if how > 2 {
        return Err(EINVAL);
    }
    crate::net::shutdown(&s.socket)
        .map(|()| 0)
        .map_err(|_| ENOTCONN)
}

/// `getsockname` (`peer` false) and `getpeername` (`peer` true).
pub fn name(p: &Process, fd: u64, addr: u64, len: u64, peer: bool) -> LinuxResult {
    let s = socket_of(p, fd)?;
    let (ip, port) = if peer {
        crate::net::peer_addr(&s.socket).map_err(|_| ENOTCONN)?
    } else {
        crate::net::local_addr(&s.socket).map_err(errno::from_error)?
    };
    write_addr(p, addr, len, ip, port)?;
    Ok(0)
}

/// `setsockopt`: accepted and ignored (`SO_REUSEADDR`, `TCP_NODELAY`...).
pub fn setsockopt(p: &Process, fd: u64) -> LinuxResult {
    socket_of(p, fd).map(|_| 0)
}

/// `getsockopt`: every option reads as 0 (`SO_ERROR`: no error).
pub fn getsockopt(p: &Process, fd: u64, value: u64, len_ptr: u64) -> LinuxResult {
    socket_of(p, fd)?;
    if value == 0 || len_ptr == 0 {
        return Err(EINVAL);
    }
    let room = u32::from_le_bytes(read_user(p, len_ptr, 4)?.try_into().map_err(|_| EBADF)?);
    if room >= 4 {
        write_user(p, value, &0u32.to_le_bytes())?;
        write_user(p, len_ptr, &4u32.to_le_bytes())?;
    }
    Ok(0)
}
