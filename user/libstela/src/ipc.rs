// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handles, ports and messages (`docs/ipc.md`).

use crate::sys::{syscall0, syscall1, syscall2, syscall3};
use abi::{Error, Handle, Message, Rights, Syscall};

/// `port_receive` timeout: wait as long as it takes.
pub const FOREVER: u64 = u64::MAX;

/// Removes `handle` from this process's table.
pub fn handle_close(handle: Handle) -> Result<(), Error> {
    syscall1(Syscall::HandleClose, handle.raw()).map(|_| ())
}

/// A new handle to the same object with `rights` (a subset of the
/// original's; the original needs `DUPLICATE`).
pub fn handle_duplicate(handle: Handle, rights: Rights) -> Result<Handle, Error> {
    syscall2(Syscall::HandleDuplicate, handle.raw(), rights.raw()).map(Handle::from_raw)
}

/// A new port; the handle has `SEND | RECEIVE | DUPLICATE | TRANSFER`.
pub fn port_create() -> Result<Handle, Error> {
    syscall0(Syscall::PortCreate).map(Handle::from_raw)
}

/// Queues a copy of `msg` on `port`; the handles it carries leave this
/// process. Never blocks (`Full` when the queue is full).
pub fn port_send(port: Handle, msg: &Message) -> Result<(), Error> {
    syscall2(Syscall::PortSend, port.raw(), msg as *const Message as u64).map(|_| ())
}

/// Takes the oldest message of `port` into `msg`, waiting up to
/// `timeout_ns` (0: not at all; [`FOREVER`]). The handles it carries are
/// in `msg.handle_slots`.
pub fn port_receive(port: Handle, msg: &mut Message, timeout_ns: u64) -> Result<(), Error> {
    syscall3(
        Syscall::PortReceive,
        port.raw(),
        msg as *mut Message as u64,
        timeout_ns,
    )
    .map(|_| ())
}

/// New zeroed shared memory of `size` bytes (a multiple of 4096, at most
/// 64 MiB); the handle has `READ | WRITE | MAP | DUPLICATE | TRANSFER`.
pub fn memory_create(size: u64) -> Result<Handle, Error> {
    syscall1(Syscall::MemoryCreate, size).map(Handle::from_raw)
}

/// Maps all of `memory` (writable with `writable`, which needs `WRITE`)
/// and returns the address.
pub fn memory_map(memory: Handle, writable: bool) -> Result<u64, Error> {
    let flags = if writable { abi::map::WRITE } else { 0 };
    syscall2(Syscall::MemoryMap, memory.raw(), flags)
}

/// Removes the mapping that `memory_map` returned `addr` for.
pub fn memory_unmap(addr: u64) -> Result<(), Error> {
    syscall1(Syscall::MemoryUnmap, addr).map(|_| ())
}

/// A shared-memory mapping, accessed by copying (another process may write
/// the same memory at any time, so no Rust reference into it is handed
/// out). Unmapped when dropped.
pub struct Mapping {
    addr: u64,
    size: u64,
    writable: bool,
}

impl Mapping {
    /// Maps `memory`, whose size is `size` bytes (as created; a larger
    /// value makes accesses past the real end fault).
    pub fn new(memory: Handle, size: u64, writable: bool) -> Result<Mapping, Error> {
        let addr = memory_map(memory, writable)?;
        Ok(Mapping {
            addr,
            size,
            writable,
        })
    }

    /// The address it is mapped at.
    pub fn addr(&self) -> u64 {
        self.addr
    }

    /// Its size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    fn range(&self, offset: u64, len: usize) -> Result<u64, Error> {
        match offset.checked_add(len as u64) {
            Some(end) if end <= self.size => Ok(self.addr + offset),
            _ => Err(Error::InvalidArgument),
        }
    }

    /// Copies bytes at `offset` into `buf`.
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let at = self.range(offset, buf.len())?;
        // SAFETY: `at..at + buf.len()` is inside the mapping (checked
        // above), which stays mapped while `self` lives; the bytes are
        // copied as plain data even if another process writes them.
        unsafe { core::ptr::copy_nonoverlapping(at as *const u8, buf.as_mut_ptr(), buf.len()) };
        Ok(())
    }

    /// Copies `data` to `offset` (the mapping must be writable).
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<(), Error> {
        if !self.writable {
            return Err(Error::AccessDenied);
        }
        let at = self.range(offset, data.len())?;
        // SAFETY: as in `read`; the pages are mapped writable.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), at as *mut u8, data.len()) };
        Ok(())
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = memory_unmap(self.addr);
    }
}

/// Keyboard events go to `port` (a handle with `SEND`) as `INPUT_KEY`
/// messages from now on, replacing whoever was attached before.
pub fn input_attach(port: Handle) -> Result<(), Error> {
    syscall1(Syscall::InputAttach, port.raw()).map(|_| ())
}
