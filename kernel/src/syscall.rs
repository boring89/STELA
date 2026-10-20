// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! System-call dispatch: decodes the number and arguments the program
//! left in its registers (`abi`), runs the call, and puts the status in
//! RAX and the value in RDX.

use crate::task;
use abi::layout::USER_END;
use abi::{DEBUG_WRITE_MAX, Error, Syscall};
use alloc::vec;
use frame::syscall::SyscallFrame;

/// Registers [`handle`] with frame. Call once at boot.
pub fn init() {
    frame::syscall::set_handler(handle);
}

/// The handler frame calls for every `syscall`.
fn handle(frame: &mut SyscallFrame) {
    let [a0, a1, a2, ..] = frame.args();
    let result = match Syscall::from_raw(frame.number()) {
        Some(Syscall::ProcessExit) => task::exit_process(a0 as i64),
        Some(Syscall::ThreadExit) => task::exit(),
        Some(Syscall::ThreadCreate) => thread_create(a0, a1, a2),
        Some(Syscall::Yield) => {
            task::yield_now();
            Ok(0)
        }
        Some(Syscall::SleepNs) => {
            task::sleep_ns(a0);
            Ok(0)
        }
        Some(Syscall::ClockNowNs) => Ok(frame::time::now_ns()),
        Some(Syscall::DebugWrite) => debug_write(a0, a1),
        None => Err(Error::InvalidSyscall),
    };
    let (status, value) = abi::encode(result);
    frame.set_return(status, value);
}

/// `ThreadCreate(entry, stack_top, arg)`.
fn thread_create(entry: u64, stack_top: u64, arg: u64) -> Result<u64, Error> {
    if entry >= USER_END || !(16..=USER_END).contains(&stack_top) {
        return Err(Error::InvalidArgument);
    }
    let Some(process) = task::current_process() else {
        return Err(Error::NotSupported);
    };
    // As if the thread had been called: RSP + 8 is 16-byte aligned.
    let rsp = (stack_top & !15) - 8;
    match task::spawn_user(&process, entry, rsp, arg) {
        Ok(id) => Ok(id),
        Err(task::SpawnError::OutOfMemory) => Err(Error::OutOfMemory),
        // The process is being torn down; this thread is about to go too.
        Err(task::SpawnError::Exiting) => Err(Error::NotSupported),
    }
}

/// `DebugWrite(ptr, len)`: copies the bytes from the caller's address
/// space (checking every page) and prints them.
fn debug_write(ptr: u64, len: u64) -> Result<u64, Error> {
    if len > DEBUG_WRITE_MAX {
        return Err(Error::InvalidArgument);
    }
    let Some(process) = task::current_process() else {
        return Err(Error::NotSupported);
    };
    let mut buf = vec![0u8; len as usize];
    process
        .space()
        .read(ptr, &mut buf)
        .map_err(|_| Error::BadAddress)?;
    for chunk in buf.utf8_chunks() {
        frame::kprint!("{}", chunk.valid());
        if !chunk.invalid().is_empty() {
            frame::kprint!("\u{FFFD}");
        }
    }
    Ok(len)
}
