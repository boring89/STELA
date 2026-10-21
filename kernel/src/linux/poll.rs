// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `poll`, `ppoll`, `select` and `pselect6` for Linux programs
//! (`docs/linux.md` §5.3), on a kernel wait set (`docs/ipc.md` §13).
//!
//! ```text
//!  loop:
//!    the events of every descriptor now (files and devices are always
//!    ready; a tty or socket asks its object)
//!    some → write them back, return how many
//!    time up → 0
//!    wait on a set of the ttys and sockets (until a change or the time)
//! ```
//!
//! Signals are never delivered, so `EINTR` only comes from a kill (and
//! the process is ending then).

use super::errno::{EBADF, EFAULT, EINTR, EINVAL};
use super::state::OpenFile;
use super::sys::{LinuxResult, file_of, read_user, write_user};
use crate::object::KernelObject;
use crate::object::waitset::WaitSet;
use crate::task::{self, Process};
use abi::{Error, Rights, wait};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

const POLLIN: i16 = 0x001;
const POLLPRI: i16 = 0x002;
const POLLOUT: i16 = 0x004;
const POLLERR: i16 = 0x008;
const POLLHUP: i16 = 0x010;
const POLLNVAL: i16 = 0x020;
/// Most descriptors one call looks at (Linux's `RLIMIT_NOFILE` default).
const MAX_FDS: usize = 1024;

/// One `struct pollfd`: fd, wanted events, returned events.
#[derive(Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

/// The events of `file` that hold now (`POLLERR`, `POLLHUP` always
/// count), and the object to wait on if it can change.
fn file_events(file: &OpenFile) -> (i16, Option<KernelObject>) {
    match file {
        OpenFile::Tty(tty) => {
            let read = if tty.readable() { POLLIN } else { 0 };
            (read | POLLOUT, Some(KernelObject::Tty(Arc::clone(tty))))
        }
        OpenFile::Socket(s) => {
            let socket = s.kernel_socket();
            let e = crate::net::events(socket);
            let mut r = 0;
            if e & wait::READABLE != 0 {
                r |= POLLIN;
            }
            if e & wait::WRITABLE != 0 {
                r |= POLLOUT;
            }
            if e & wait::CLOSED != 0 {
                r |= POLLHUP;
            }
            (r, Some(KernelObject::Socket(Arc::clone(socket))))
        }
        OpenFile::PipeRead(end, _) => {
            let e = end.0.events(true);
            let mut r = 0;
            if e & wait::READABLE != 0 {
                r |= POLLIN;
            }
            if e & wait::CLOSED != 0 {
                r |= POLLHUP;
            }
            (r, Some(KernelObject::PipeRead(Arc::clone(end))))
        }
        OpenFile::PipeWrite(end, _) => {
            let e = end.0.events(false);
            let mut r = 0;
            if e & wait::WRITABLE != 0 {
                r |= POLLOUT;
            }
            if e & wait::CLOSED != 0 {
                r |= POLLERR;
            }
            (r, Some(KernelObject::PipeWrite(Arc::clone(end))))
        }
        // Files, folders and devices never block.
        _ => (POLLIN | POLLOUT, None),
    }
}

/// Fills in `revents` of every entry; returns how many have some, and
/// the objects to wait on.
fn scan(p: &Process, fds: &mut [PollFd]) -> (usize, Vec<KernelObject>) {
    let mut ready = 0;
    let mut objects = Vec::new();
    for f in fds.iter_mut() {
        f.revents = 0;
        if f.fd < 0 {
            continue;
        }
        match file_of(p, f.fd as u64) {
            Err(_) => f.revents = POLLNVAL,
            Ok(file) => {
                let (now, object) = file_events(&file);
                f.revents = now & (f.events | POLLERR | POLLHUP);
                if let Some(o) = object {
                    objects.push(o);
                }
            }
        }
        if f.revents != 0 {
            ready += 1;
        }
    }
    (ready, objects)
}

/// Waits until some entry of `fds` is ready or `deadline` (`now_ns`;
/// `None`: forever); returns how many are.
fn poll_fds(p: &Process, fds: &mut [PollFd], deadline: Option<u64>) -> Result<usize, i64> {
    loop {
        if task::current_killed() {
            return Err(EINTR);
        }
        let (ready, objects) = scan(p, fds);
        if ready > 0 || deadline.is_some_and(|d| frame::time::now_ns() >= d) {
            return Ok(ready);
        }
        if objects.is_empty() {
            // Nothing can change: just sleep (in steps, to notice a kill).
            let step = frame::time::now_ns() + 100_000_000;
            let until = deadline.map_or(step, |d| d.min(step));
            task::block_until(Some(until));
            continue;
        }
        let set = WaitSet::new();
        for (key, o) in objects.iter().enumerate() {
            // Ttys and sockets: the rights do not matter.
            let _ = set.add(o, Rights::READ, key as u64, 0);
        }
        match set.wait(deadline, 1) {
            Ok(_) | Err(Error::TimedOut) => {}
            Err(_) => return Err(EINTR),
        }
    }
}

/// A deadline `ns` from now.
fn after(ns: u64) -> Option<u64> {
    Some(frame::time::now_ns().saturating_add(ns))
}

/// A `struct timespec` of the caller as a deadline (`ptr` 0: none).
fn timespec_deadline(p: &Process, ptr: u64) -> Result<Option<u64>, i64> {
    if ptr == 0 {
        return Ok(None);
    }
    let b = read_user(p, ptr, 16)?;
    let sec = i64::from_le_bytes(b[..8].try_into().map_err(|_| EFAULT)?);
    let nsec = i64::from_le_bytes(b[8..].try_into().map_err(|_| EFAULT)?);
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(EINVAL);
    }
    Ok(after(
        (sec as u64).saturating_mul(1_000_000_000) + nsec as u64,
    ))
}

fn read_pollfds(p: &Process, ptr: u64, n: u64) -> Result<Vec<PollFd>, i64> {
    if n as usize > MAX_FDS {
        return Err(EINVAL);
    }
    let raw = read_user(p, ptr, n as usize * 8)?;
    Ok(raw
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| PollFd {
            fd: i32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            events: i16::from_le_bytes([c[4], c[5]]),
            revents: 0,
        })
        .collect())
}

fn poll_common(p: &Process, ptr: u64, n: u64, deadline: Option<u64>) -> LinuxResult {
    let mut fds = read_pollfds(p, ptr, n)?;
    let ready = poll_fds(p, &mut fds, deadline)?;
    for (i, f) in fds.iter().enumerate() {
        write_user(p, ptr + i as u64 * 8 + 6, &f.revents.to_le_bytes())?;
    }
    Ok(ready as u64)
}

/// `poll(fds, nfds, timeout_ms)`: a negative timeout waits forever.
pub fn poll(p: &Process, ptr: u64, n: u64, timeout_ms: u64) -> LinuxResult {
    let t = timeout_ms as i32;
    let deadline = if t < 0 {
        None
    } else {
        after(t as u64 * 1_000_000)
    };
    poll_common(p, ptr, n, deadline)
}

/// `ppoll(fds, nfds, timeout, sigmask, size)`: the mask is ignored.
pub fn ppoll(p: &Process, ptr: u64, n: u64, timeout: u64) -> LinuxResult {
    let deadline = timespec_deadline(p, timeout)?;
    poll_common(p, ptr, n, deadline)
}

/// The words of an `fd_set` for `nfds` descriptors (`ptr` 0: none).
fn read_set(p: &Process, ptr: u64, nfds: usize) -> Result<Option<Vec<u64>>, i64> {
    if ptr == 0 {
        return Ok(None);
    }
    let words = nfds.div_ceil(64);
    let raw = read_user(p, ptr, words * 8)?;
    Ok(Some(
        raw.as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect(),
    ))
}

fn has(set: &Option<Vec<u64>>, fd: usize) -> bool {
    set.as_ref()
        .is_some_and(|s| s[fd / 64] & (1 << (fd % 64)) != 0)
}

fn write_set(p: &Process, ptr: u64, set: &Option<Vec<u64>>) -> Result<(), i64> {
    if let Some(words) = set {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        write_user(p, ptr, &bytes)?;
    }
    Ok(())
}

/// `select` and `pselect6`: the three sets become poll entries; the
/// result is the number of bits set.
fn select_common(p: &Process, args: [u64; 4], deadline: Option<u64>) -> LinuxResult {
    let [nfds, rp, wp, ep] = args;
    let nfds = nfds as usize;
    if nfds > MAX_FDS {
        return Err(EINVAL);
    }
    let (r, w, e) = (
        read_set(p, rp, nfds)?,
        read_set(p, wp, nfds)?,
        read_set(p, ep, nfds)?,
    );
    let mut fds = Vec::new();
    for fd in 0..nfds {
        let mut events = 0;
        if has(&r, fd) {
            events |= POLLIN;
        }
        if has(&w, fd) {
            events |= POLLOUT;
        }
        if has(&e, fd) {
            events |= POLLPRI;
        }
        if events != 0 {
            if file_of(p, fd as u64).is_err() {
                return Err(EBADF);
            }
            fds.push(PollFd {
                fd: fd as i32,
                events,
                revents: 0,
            });
        }
    }
    poll_fds(p, &mut fds, deadline)?;
    let words = nfds.div_ceil(64);
    let (mut ro, mut wo, mut eo) = (vec![0u64; words], vec![0u64; words], vec![0u64; words]);
    let mut count = 0;
    for f in &fds {
        let (i, bit) = (f.fd as usize / 64, 1u64 << (f.fd as usize % 64));
        if f.events & POLLIN != 0 && f.revents & (POLLIN | POLLHUP | POLLERR) != 0 {
            ro[i] |= bit;
            count += 1;
        }
        if f.events & POLLOUT != 0 && f.revents & (POLLOUT | POLLERR) != 0 {
            wo[i] |= bit;
            count += 1;
        }
        if f.events & POLLPRI != 0 && f.revents & POLLPRI != 0 {
            eo[i] |= bit;
            count += 1;
        }
    }
    write_set(p, rp, &r.map(|_| ro))?;
    write_set(p, wp, &w.map(|_| wo))?;
    write_set(p, ep, &e.map(|_| eo))?;
    Ok(count)
}

/// `select(nfds, readfds, writefds, exceptfds, timeval)`.
pub fn select(p: &Process, args: [u64; 5]) -> LinuxResult {
    let [nfds, r, w, e, tv] = args;
    let deadline = if tv == 0 {
        None
    } else {
        let b = read_user(p, tv, 16)?;
        let sec = i64::from_le_bytes(b[..8].try_into().map_err(|_| EFAULT)?);
        let usec = i64::from_le_bytes(b[8..].try_into().map_err(|_| EFAULT)?);
        if sec < 0 || !(0..1_000_000).contains(&usec) {
            return Err(EINVAL);
        }
        after((sec as u64).saturating_mul(1_000_000_000) + usec as u64 * 1000)
    };
    select_common(p, [nfds, r, w, e], deadline)
}

/// `pselect6(nfds, readfds, writefds, exceptfds, timespec, sigmask)`: the
/// mask is ignored.
pub fn pselect6(p: &Process, args: [u64; 5]) -> LinuxResult {
    let [nfds, r, w, e, ts] = args;
    let deadline = timespec_deadline(p, ts)?;
    select_common(p, [nfds, r, w, e], deadline)
}
