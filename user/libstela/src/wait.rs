// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Waiting on several objects at once (`docs/ipc.md` §13): ports,
//! sockets, terminals, processes, the sound output.

use crate::sys::{syscall0, syscall2, syscall4};
use abi::wait::{READY_SIZE, Ready};
use abi::{Error, Handle, Syscall};

pub use abi::wait::{ALL, CLOSED, CONNECTED, EXITED, READABLE, WRITABLE};

/// A wait set; closed when dropped.
pub struct WaitSet {
    handle: Handle,
}

impl WaitSet {
    /// An empty set.
    pub fn new() -> Result<WaitSet, Error> {
        syscall0(Syscall::WaitSetCreate).map(|h| WaitSet {
            handle: Handle::from_raw(h),
        })
    }

    /// The set's handle.
    pub fn handle(&self) -> Handle {
        self.handle
    }

    /// Adds `object` (or changes its key and events): it is reported with
    /// `key` when one of the events in `mask` (0: all) holds.
    pub fn add(&self, object: Handle, key: u64, mask: u32) -> Result<(), Error> {
        syscall4(
            Syscall::WaitSetAdd,
            self.handle.raw(),
            object.raw(),
            key,
            u64::from(mask),
        )
        .map(|_| ())
    }

    /// Removes `object`.
    pub fn remove(&self, object: Handle) -> Result<(), Error> {
        syscall2(Syscall::WaitSetRemove, self.handle.raw(), object.raw()).map(|_| ())
    }

    /// Waits up to `timeout_ns` (`ipc::FOREVER`: forever; 0: just look)
    /// until some member is ready; fills `out` with the ready ones and
    /// returns how many. `TimedOut` when none became ready.
    pub fn wait(&self, out: &mut [Ready], timeout_ns: u64) -> Result<usize, Error> {
        let mut raw = [[0u8; READY_SIZE]; abi::wait::MAX_MEMBERS];
        let max = out.len().min(raw.len());
        if max == 0 {
            return Err(Error::InvalidArgument);
        }
        let n = syscall4(
            Syscall::WaitSetWait,
            self.handle.raw(),
            raw.as_mut_ptr() as u64,
            max as u64,
            timeout_ns,
        )? as usize;
        let n = n.min(max);
        for (o, r) in out.iter_mut().zip(&raw[..n]) {
            *o = Ready::from_bytes(r);
        }
        Ok(n)
    }
}

impl Drop for WaitSet {
    fn drop(&mut self) {
        let _ = crate::ipc::handle_close(self.handle);
    }
}
