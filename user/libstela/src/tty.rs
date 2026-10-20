// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Terminals (`docs/linux.md` §5): the program side is used like the
//! console (send `CONSOLE_WRITE` to print, `INPUT_KEY` to type); the
//! master side is the terminal application's.

use crate::sys::{syscall1, syscall3, syscall4};
use abi::{Error, Handle, Syscall};

/// A new terminal whose output goes to the kernel console: its program
/// side.
pub fn create() -> Result<Handle, Error> {
    syscall1(Syscall::TtyCreate, 0).map(|v| Handle::from_raw(v & 0xFFFF_FFFF))
}

/// A new terminal with a master end: (program side, master).
pub fn create_with_master() -> Result<(Handle, Handle), Error> {
    syscall1(Syscall::TtyCreate, abi::tty::MASTER)
        .map(|v| (Handle::from_raw(v & 0xFFFF_FFFF), Handle::from_raw(v >> 32)))
}

/// The terminal's output (up to 4096 bytes), waiting up to `timeout_ns`.
pub fn read(master: Handle, buf: &mut [u8], timeout_ns: u64) -> Result<usize, Error> {
    syscall4(
        Syscall::TtyRead,
        master.raw(),
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
        timeout_ns,
    )
    .map(|n| n as usize)
}

/// Types `text` into the terminal (up to 4096 bytes).
pub fn write(master: Handle, text: &[u8]) -> Result<usize, Error> {
    syscall3(
        Syscall::TtyWrite,
        master.raw(),
        text.as_ptr() as u64,
        text.len() as u64,
    )
    .map(|n| n as usize)
}
