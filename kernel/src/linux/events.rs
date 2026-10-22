// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `eventfd`, `timerfd` and `epoll` for Linux programs (P13 K6,
//! `docs/linux.md` §16.7).
//!
//! ```text
//!  eventfd   a counter; readable while it is not 0. A pipe holds one
//!            byte while it is not, so poll and epoll can wait on it.
//!  timerfd   a deadline (now_ns) and an interval; expirations are
//!            counted when someone looks. poll and epoll wake at the
//!            deadline (no kernel timer).
//!  epoll     the descriptors of interest; epoll_wait is poll over them
//!            (level-triggered; EPOLLET is taken as level, EPOLLONESHOT
//!            turns the entry off after it is reported).
//! ```

use super::errno::{EAGAIN, EBADF, EEXIST, EFAULT, EINTR, EINVAL, ENOENT};
use super::state::OpenFile;
use super::sys::{LinuxResult, file_of, read_user, write_user};
use crate::object::KernelObject;
use crate::object::pipe::{Pipe, ReadEnd, WriteEnd};
use crate::object::waitset::WaitSet;
use crate::task::{self, Process};
use abi::Rights;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use frame::sync::SpinLock;

const EFD_SEMAPHORE: u64 = 1;
const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;
const TFD_TIMER_ABSTIME: u64 = 1;
const CLOCK_REALTIME: u64 = 0;

const EPOLL_CTL_ADD: u64 = 1;
const EPOLL_CTL_DEL: u64 = 2;
const EPOLL_CTL_MOD: u64 = 3;
/// `EPOLLONESHOT`.
const EPOLLONESHOT: u32 = 1 << 30;
/// The event bits that are the same as poll's (IN, PRI, OUT, ERR, HUP).
const EPOLL_POLL_BITS: u32 = 0x1F;
/// `struct epoll_event` on x86-64 is packed: u32 events, u64 data.
const EPOLL_EVENT_SIZE: usize = 12;

/// An `eventfd`.
pub struct EventFd {
    counter: SpinLock<u64>,
    semaphore: bool,
    nonblock: AtomicBool,
    /// Holds one byte while the counter is not 0.
    signal_read: Arc<ReadEnd>,
    signal_write: Arc<WriteEnd>,
}

impl EventFd {
    /// Whether a read would not wait.
    pub fn readable(&self) -> bool {
        *self.counter.lock() > 0
    }

    /// The object poll waits on.
    pub fn wait_object(&self) -> KernelObject {
        KernelObject::PipeRead(Arc::clone(&self.signal_read))
    }

    /// `read`: the counter (or 1 for a semaphore), as 8 bytes.
    pub fn read(&self, len: usize) -> Result<Vec<u8>, i64> {
        if len < 8 {
            return Err(EINVAL);
        }
        loop {
            {
                let mut c = self.counter.lock();
                if *c > 0 {
                    let v = if self.semaphore { 1 } else { *c };
                    *c -= v;
                    if *c == 0 {
                        let _ = self.signal_read.0.read(16, Some(0));
                    }
                    return Ok(v.to_le_bytes().to_vec());
                }
            }
            if self.nonblock.load(Ordering::Relaxed) {
                return Err(EAGAIN);
            }
            wait_on(&self.wait_object(), None)?;
        }
    }

    /// `write`: adds an 8-byte value.
    pub fn write(&self, data: &[u8]) -> Result<usize, i64> {
        let bytes: [u8; 8] = data
            .get(..8)
            .ok_or(EINVAL)?
            .try_into()
            .map_err(|_| EINVAL)?;
        let v = u64::from_le_bytes(bytes);
        if v == u64::MAX {
            return Err(EINVAL);
        }
        let mut c = self.counter.lock();
        if *c == 0 && v > 0 {
            let _ = self.signal_write.0.write(&[1], Some(0));
        }
        *c = c.saturating_add(v).min(u64::MAX - 1);
        Ok(8)
    }

    /// `O_NONBLOCK` through `fcntl`.
    pub fn set_nonblock(&self, on: bool) {
        self.nonblock.store(on, Ordering::Relaxed);
    }
}

/// Waits until `object` changes (or `deadline`); `EINTR` on a kill.
fn wait_on(object: &KernelObject, deadline: Option<u64>) -> Result<(), i64> {
    if task::current_killed() {
        return Err(EINTR);
    }
    let set = WaitSet::new();
    let _ = set.add(object, Rights::READ, 0, 0);
    // Wake now and then to notice a kill.
    let step = frame::time::now_ns() + 100_000_000;
    let until = deadline.map_or(step, |d| d.min(step));
    let _ = set.wait(Some(until), 1);
    Ok(())
}

/// `eventfd2(initval, flags)` (and `eventfd`).
pub fn eventfd(p: &Process, initval: u64, flags: u64) -> LinuxResult {
    if flags & !(EFD_SEMAPHORE | O_NONBLOCK | O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let (r, w) = Pipe::create();
    let e = EventFd {
        counter: SpinLock::new(initval & 0xFFFF_FFFF),
        semaphore: flags & EFD_SEMAPHORE != 0,
        nonblock: AtomicBool::new(flags & O_NONBLOCK != 0),
        signal_read: r,
        signal_write: w,
    };
    if *e.counter.lock() > 0 {
        let _ = e.signal_write.0.write(&[1], Some(0));
    }
    super::files::add_fd(p, OpenFile::EventFd(Arc::new(e)), flags & O_CLOEXEC != 0)
}

/// A `timerfd`.
pub struct TimerFd {
    state: SpinLock<TimerState>,
    nonblock: AtomicBool,
}

struct TimerState {
    /// Next expiry (`now_ns`); `None`: disarmed.
    deadline: Option<u64>,
    /// 0: one shot.
    interval: u64,
    /// Expirations not read yet.
    expired: u64,
}

impl TimerState {
    /// Counts the expirations up to `now`.
    fn update(&mut self, now: u64) {
        let Some(d) = self.deadline else {
            return;
        };
        if now < d {
            return;
        }
        match (now - d).checked_div(self.interval) {
            Some(q) => {
                let n = q + 1;
                self.expired = self.expired.saturating_add(n);
                self.deadline = Some(d + n * self.interval);
            }
            // One shot (no interval).
            None => {
                self.expired = self.expired.saturating_add(1);
                self.deadline = None;
            }
        }
    }
}

impl TimerFd {
    /// Whether a read would not wait, and when that changes.
    pub fn status(&self) -> (bool, Option<u64>) {
        let mut s = self.state.lock();
        s.update(frame::time::now_ns());
        (s.expired > 0, s.deadline)
    }

    /// `read`: the expirations since the last read, as 8 bytes.
    pub fn read(&self, len: usize) -> Result<Vec<u8>, i64> {
        if len < 8 {
            return Err(EINVAL);
        }
        loop {
            let deadline = {
                let mut s = self.state.lock();
                s.update(frame::time::now_ns());
                if s.expired > 0 {
                    let v = core::mem::take(&mut s.expired);
                    return Ok(v.to_le_bytes().to_vec());
                }
                s.deadline
            };
            if self.nonblock.load(Ordering::Relaxed) {
                return Err(EAGAIN);
            }
            if task::current_killed() {
                return Err(EINTR);
            }
            let step = frame::time::now_ns() + 100_000_000;
            task::block_until(Some(deadline.map_or(step, |d| d.min(step))));
        }
    }

    /// `O_NONBLOCK` through `fcntl`.
    pub fn set_nonblock(&self, on: bool) {
        self.nonblock.store(on, Ordering::Relaxed);
    }
}

/// `timerfd_create(clock, flags)`.
pub fn timerfd_create(p: &Process, clock: u64, flags: u64) -> LinuxResult {
    if clock > 7 || flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let t = TimerFd {
        state: SpinLock::new(TimerState {
            deadline: None,
            interval: 0,
            expired: 0,
        }),
        nonblock: AtomicBool::new(flags & O_NONBLOCK != 0),
    };
    // The clock only matters for absolute times (`timerfd_settime`): it
    // is remembered as the realtime flag in the interval's state.
    let fd = super::files::add_fd(
        p,
        OpenFile::TimerFd(Arc::new(t), clock == CLOCK_REALTIME),
        flags & O_CLOEXEC != 0,
    )?;
    Ok(fd)
}

fn timer_of(p: &Process, fd: u64) -> Result<(Arc<OpenFile>, bool), i64> {
    let file = file_of(p, fd)?;
    let realtime = match &*file {
        OpenFile::TimerFd(_, r) => *r,
        _ => return Err(EINVAL),
    };
    Ok((file, realtime))
}

fn timespec_ns(b: &[u8]) -> Result<u64, i64> {
    let sec = i64::from_le_bytes(b[..8].try_into().map_err(|_| EFAULT)?);
    let nsec = i64::from_le_bytes(b[8..16].try_into().map_err(|_| EFAULT)?);
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(EINVAL);
    }
    Ok((sec as u64).saturating_mul(1_000_000_000) + nsec as u64)
}

fn put_itimerspec(p: &Process, ptr: u64, interval: u64, value: u64) -> Result<(), i64> {
    let mut b = [0u8; 32];
    for (i, v) in [interval, value].into_iter().enumerate() {
        b[i * 16..i * 16 + 8].copy_from_slice(&(v / 1_000_000_000).to_le_bytes());
        b[i * 16 + 8..i * 16 + 16].copy_from_slice(&(v % 1_000_000_000).to_le_bytes());
    }
    write_user(p, ptr, &b)
}

/// The interval and the time left of a timer.
fn current(t: &TimerFd) -> (u64, u64) {
    let now = frame::time::now_ns();
    let mut s = t.state.lock();
    s.update(now);
    (s.interval, s.deadline.map_or(0, |d| d.saturating_sub(now)))
}

/// `timerfd_settime(fd, flags, new, old)`.
pub fn timerfd_settime(p: &Process, fd: u64, flags: u64, new: u64, old: u64) -> LinuxResult {
    let (file, realtime) = timer_of(p, fd)?;
    let OpenFile::TimerFd(t, _) = &*file else {
        return Err(EINVAL);
    };
    let b = read_user(p, new, 32)?;
    let interval = timespec_ns(&b[..16])?;
    let value = timespec_ns(&b[16..])?;
    if old != 0 {
        let (i, v) = current(t);
        put_itimerspec(p, old, i, v)?;
    }
    let now = frame::time::now_ns();
    let deadline = if value == 0 {
        None
    } else if flags & TFD_TIMER_ABSTIME != 0 {
        // An absolute time on the timer's clock, as now_ns.
        let clock_now = if realtime {
            frame::time::wall_clock_ns()
        } else {
            now
        };
        Some(now + value.saturating_sub(clock_now))
    } else {
        Some(now + value)
    };
    let mut s = t.state.lock();
    s.deadline = deadline;
    s.interval = interval;
    s.expired = 0;
    Ok(0)
}

/// `timerfd_gettime(fd, cur)`.
pub fn timerfd_gettime(p: &Process, fd: u64, cur: u64) -> LinuxResult {
    let (file, _) = timer_of(p, fd)?;
    let OpenFile::TimerFd(t, _) = &*file else {
        return Err(EINVAL);
    };
    let (i, v) = current(t);
    put_itimerspec(p, cur, i, v)?;
    Ok(0)
}

/// An epoll instance: descriptor → (events, data).
pub struct Epoll {
    interest: SpinLock<BTreeMap<i32, (u32, u64)>>,
}

/// `epoll_create1(flags)` (and `epoll_create(size)`).
pub fn epoll_create(p: &Process, flags: u64) -> LinuxResult {
    if flags & !O_CLOEXEC != 0 {
        return Err(EINVAL);
    }
    let e = Epoll {
        interest: SpinLock::new(BTreeMap::new()),
    };
    super::files::add_fd(p, OpenFile::Epoll(Arc::new(e)), flags & O_CLOEXEC != 0)
}

fn epoll_of(p: &Process, fd: u64) -> Result<Arc<OpenFile>, i64> {
    let file = file_of(p, fd)?;
    if !matches!(&*file, OpenFile::Epoll(_)) {
        return Err(EINVAL);
    }
    Ok(file)
}

/// `epoll_ctl(epfd, op, fd, event)`.
pub fn epoll_ctl(p: &Process, epfd: u64, op: u64, fd: u64, event: u64) -> LinuxResult {
    let file = epoll_of(p, epfd)?;
    let OpenFile::Epoll(e) = &*file else {
        return Err(EINVAL);
    };
    if fd == epfd {
        return Err(EINVAL);
    }
    // The descriptor must be open.
    file_of(p, fd).map_err(|_| EBADF)?;
    let fd = fd as i32;
    let mut interest = e.interest.lock();
    match op {
        EPOLL_CTL_DEL => {
            interest.remove(&fd).ok_or(ENOENT)?;
        }
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            let b = read_user(p, event, EPOLL_EVENT_SIZE)?;
            let events = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            let data = u64::from_le_bytes(b[4..12].try_into().map_err(|_| EFAULT)?);
            let exists = interest.contains_key(&fd);
            if op == EPOLL_CTL_ADD && exists {
                return Err(EEXIST);
            }
            if op == EPOLL_CTL_MOD && !exists {
                return Err(ENOENT);
            }
            interest.insert(fd, (events, data));
        }
        _ => return Err(EINVAL),
    }
    Ok(0)
}

/// `epoll_wait(epfd, events, maxevents, timeout_ms)` (and `epoll_pwait`,
/// whose mask is ignored).
pub fn epoll_wait(p: &Process, epfd: u64, out: u64, max: u64, timeout_ms: u64) -> LinuxResult {
    let file = epoll_of(p, epfd)?;
    let OpenFile::Epoll(e) = &*file else {
        return Err(EINVAL);
    };
    let max = max as i32;
    if max <= 0 {
        return Err(EINVAL);
    }
    let t = timeout_ms as i32;
    let deadline = (t >= 0).then(|| frame::time::now_ns().saturating_add(t as u64 * 1_000_000));
    let entries: Vec<(i32, u32, u64)> = {
        let mut interest = e.interest.lock();
        // Closed descriptors leave the set (as when Linux closes the file).
        interest.retain(|&fd, _| file_of(p, fd as u64).is_ok());
        interest
            .iter()
            .map(|(&fd, &(ev, data))| (fd, ev, data))
            .collect()
    };
    let mut fds: Vec<super::poll::PollFd> = entries
        .iter()
        .map(|&(fd, ev, _)| super::poll::PollFd::new(fd, (ev & EPOLL_POLL_BITS) as i16))
        .collect();
    super::poll::poll_fds(p, &mut fds, deadline)?;
    let mut bytes = Vec::new();
    let mut count = 0;
    let mut oneshot = Vec::new();
    for (f, &(fd, ev, data)) in fds.iter().zip(&entries) {
        if f.revents() == 0 || count >= max {
            continue;
        }
        bytes.extend_from_slice(&(f.revents() as u16 as u32).to_le_bytes());
        bytes.extend_from_slice(&data.to_le_bytes());
        count += 1;
        if ev & EPOLLONESHOT != 0 {
            oneshot.push(fd);
        }
    }
    if !oneshot.is_empty() {
        let mut interest = e.interest.lock();
        for fd in oneshot {
            if let Some(entry) = interest.get_mut(&fd) {
                entry.0 &= !EPOLL_POLL_BITS;
            }
        }
    }
    write_user(p, out, &bytes)?;
    Ok(count as u64)
}
