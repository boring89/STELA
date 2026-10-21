// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `futex` (P12, `docs/linux.md` §11.2): a per-process list of waiting
//! threads keyed by the user address. The value check and the queueing
//! happen under the list's lock, and wakers take the same lock, so a wake
//! between "the value is still X" and "asleep" is never lost:
//!
//! ```text
//!  FUTEX_WAIT(addr, val)              FUTEX_WAKE(addr, n)
//!  lock                               (the waker changed *addr first)
//!  *addr != val → EAGAIN              lock
//!  queue (addr, me); unlock           take up to n waiters of addr; unlock
//!  block until woken / timeout ◄───── wake each
//!  still queued? → ETIMEDOUT / EINTR, else 0
//! ```
//!
//! Private and shared futexes are the same (no process shares memory with
//! another here).

use super::errno::{EAGAIN, EINTR, EINVAL, ENOSYS, ETIMEDOUT};
use super::state::FutexWaiter;
use super::sys::{LinuxResult, read_u64, read_user};
use crate::task::{self, Process};
use alloc::vec::Vec;

const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const FUTEX_REQUEUE: u64 = 3;
const FUTEX_CMP_REQUEUE: u64 = 4;
const FUTEX_WAIT_BITSET: u64 = 9;
const FUTEX_WAKE_BITSET: u64 = 10;
const FUTEX_CMD_MASK: u64 = 0x7F;
const FUTEX_CLOCK_REALTIME: u64 = 256;

fn read_u32(p: &Process, addr: u64) -> Result<u32, i64> {
    let b = read_user(p, addr, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// A `struct timespec` in nanoseconds.
fn timespec(p: &Process, ptr: u64) -> Result<u64, i64> {
    let sec = read_u64(p, ptr)? as i64;
    let nsec = read_u64(p, ptr + 8)? as i64;
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(EINVAL);
    }
    Ok((sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64))
}

/// `futex(uaddr, op, val, timeout/val2, uaddr2, val3)`.
pub fn futex(p: &Process, args: [u64; 6]) -> LinuxResult {
    let [uaddr, op, val, timeout, uaddr2, val3] = args;
    match op & FUTEX_CMD_MASK {
        FUTEX_WAIT => {
            let deadline = if timeout == 0 {
                None
            } else {
                Some(frame::time::now_ns().saturating_add(timespec(p, timeout)?))
            };
            wait(p, uaddr, val as u32, u32::MAX, deadline)
        }
        FUTEX_WAIT_BITSET => {
            if val3 as u32 == 0 {
                return Err(EINVAL);
            }
            // An absolute time on CLOCK_MONOTONIC (or REALTIME).
            let deadline = if timeout == 0 {
                None
            } else {
                let at = timespec(p, timeout)?;
                let now = if op & FUTEX_CLOCK_REALTIME != 0 {
                    frame::time::wall_clock_ns()
                } else {
                    frame::time::now_ns()
                };
                Some(frame::time::now_ns().saturating_add(at.saturating_sub(now)))
            };
            wait(p, uaddr, val as u32, val3 as u32, deadline)
        }
        FUTEX_WAKE => Ok(wake(p, uaddr, val as usize, u32::MAX)),
        FUTEX_WAKE_BITSET => {
            if val3 as u32 == 0 {
                return Err(EINVAL);
            }
            Ok(wake(p, uaddr, val as usize, val3 as u32))
        }
        FUTEX_REQUEUE => requeue(p, uaddr, val as usize, timeout as usize, uaddr2, None),
        FUTEX_CMP_REQUEUE => requeue(
            p,
            uaddr,
            val as usize,
            timeout as usize,
            uaddr2,
            Some(val3 as u32),
        ),
        _ => Err(ENOSYS),
    }
}

/// `FUTEX_WAIT`: sleeps while `*uaddr == val` until woken through a
/// matching bit of `bitset`, `deadline` or a signal.
fn wait(p: &Process, uaddr: u64, val: u32, bitset: u32, deadline: Option<u64>) -> LinuxResult {
    if !uaddr.is_multiple_of(4) {
        return Err(EINVAL);
    }
    let linux = p.linux().ok_or(EINVAL)?;
    let me = task::current_id();
    {
        let mut q = linux.futex.lock();
        if read_u32(p, uaddr)? != val {
            return Err(EAGAIN);
        }
        q.push(FutexWaiter {
            addr: uaddr,
            bitset,
            thread: me,
        });
    }
    loop {
        let queued = linux.futex.lock().iter().any(|w| w.thread == me);
        if !queued {
            return Ok(0);
        }
        let timed_out = deadline.is_some_and(|d| frame::time::now_ns() >= d);
        if timed_out || task::current_killed() {
            let mut q = linux.futex.lock();
            let before = q.len();
            q.retain(|w| w.thread != me);
            if q.len() == before {
                // Woken at the same moment: that counts.
                return Ok(0);
            }
            return Err(if timed_out { ETIMEDOUT } else { EINTR });
        }
        task::block_until(deadline);
    }
}

/// `FUTEX_WAKE`: wakes up to `n` threads waiting on `uaddr` (with a bit
/// of `bitset`); returns how many.
pub fn wake(p: &Process, uaddr: u64, n: usize, bitset: u32) -> u64 {
    let Some(linux) = p.linux() else {
        return 0;
    };
    let woken: Vec<task::ThreadId> = {
        let mut q = linux.futex.lock();
        let mut woken = Vec::new();
        q.retain(|w| {
            if woken.len() < n && w.addr == uaddr && w.bitset & bitset != 0 {
                woken.push(w.thread);
                false
            } else {
                true
            }
        });
        woken
    };
    for &t in &woken {
        task::wake(t);
    }
    woken.len() as u64
}

/// `FUTEX_(CMP_)REQUEUE`: wakes `n` waiters of `uaddr` and moves up to
/// `move_n` more to `uaddr2`.
fn requeue(
    p: &Process,
    uaddr: u64,
    n: usize,
    move_n: usize,
    uaddr2: u64,
    expect: Option<u32>,
) -> LinuxResult {
    let linux = p.linux().ok_or(EINVAL)?;
    let woken: Vec<task::ThreadId> = {
        let mut q = linux.futex.lock();
        if let Some(v) = expect
            && read_u32(p, uaddr)? != v
        {
            return Err(EAGAIN);
        }
        let mut woken = Vec::new();
        let mut moved = 0;
        q.retain_mut(|w| {
            if w.addr != uaddr {
                return true;
            }
            if woken.len() < n {
                woken.push(w.thread);
                return false;
            }
            if moved < move_n {
                w.addr = uaddr2;
                moved += 1;
            }
            true
        });
        woken
    };
    for &t in &woken {
        task::wake(t);
    }
    Ok(woken.len() as u64)
}
