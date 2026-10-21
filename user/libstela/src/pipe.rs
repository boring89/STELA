// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pipes (`docs/ipc.md` §14): a 64 KiB byte stream with a reading and a
//! writing end. Printing (`println!`) to a writing end given as `[1]`
//! works as to the console: the kernel takes `CONSOLE_WRITE` messages.

use crate::sys::{syscall1, syscall4};
use abi::{Error, Handle, Syscall};

/// A new pipe: (reading end, writing end).
pub fn create() -> Result<(Handle, Handle), Error> {
    let mut b = [0u8; 8];
    syscall1(Syscall::PipeCreate, b.as_mut_ptr() as u64)?;
    Ok((
        Handle(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        Handle(u32::from_le_bytes([b[4], b[5], b[6], b[7]])),
    ))
}

/// Reads what is there (up to `buf.len()`), waiting up to `timeout_ns`
/// for something; 0 at the end of the stream.
pub fn read(end: Handle, buf: &mut [u8], timeout_ns: u64) -> Result<usize, Error> {
    syscall4(
        Syscall::PipeRead,
        end.raw(),
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
        timeout_ns,
    )
    .map(|n| n as usize)
}

/// Writes as much as fits, waiting up to `timeout_ns` for room.
/// `PeerClosed` once nobody reads.
pub fn write(end: Handle, data: &[u8], timeout_ns: u64) -> Result<usize, Error> {
    syscall4(
        Syscall::PipeWrite,
        end.raw(),
        data.as_ptr() as u64,
        data.len() as u64,
        timeout_ns,
    )
    .map(|n| n as usize)
}

/// Writes all of `data`, waiting as long as it takes.
pub fn write_all(end: Handle, data: &[u8]) -> Result<(), Error> {
    let mut done = 0;
    while done < data.len() {
        done += write(end, &data[done..], crate::ipc::FOREVER)?;
    }
    Ok(())
}
