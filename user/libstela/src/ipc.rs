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
