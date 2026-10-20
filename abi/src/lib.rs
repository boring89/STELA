// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA system-call ABI, shared by the kernel and user programs.
//!
//! A program enters the kernel with the `syscall` instruction:
//!
//! ```text
//!  in:   RAX = number (Syscall)   RDI RSI RDX R10 R8 R9 = arguments 1-6
//!  out:  RAX = status (0 = OK, otherwise an Error value)
//!        RDX = the result value (meaningful when the status is OK)
//!  lost: RCX, R11 (the processor uses them for the return address and
//!        RFLAGS); every other register is preserved
//! ```
//!
//! R10 replaces RCX as the fourth argument because `syscall` overwrites
//! RCX (Intel SDM Vol. 2B "SYSCALL").

#![no_std]
#![forbid(unsafe_code)]

/// System-call numbers (RAX).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Syscall {
    /// `ProcessExit(code)`: ends every thread of the calling process. Does
    /// not return.
    ProcessExit = 0,
    /// `ThreadExit()`: ends the calling thread; the process ends with its
    /// last thread (exit code 0). Does not return.
    ThreadExit = 1,
    /// `ThreadCreate(entry, stack_top, arg) -> thread id`: a new thread of
    /// the calling process starts at `entry` with RDI = `arg` and RSP =
    /// `stack_top` rounded down to 16, minus 8 (as after a `call`).
    ThreadCreate = 2,
    /// `Yield()`: lets other threads run.
    Yield = 3,
    /// `SleepNs(ns)`: sleeps at least `ns` nanoseconds.
    SleepNs = 4,
    /// `ClockNowNs() -> ns`: nanoseconds since boot, monotonic.
    ClockNowNs = 5,
    /// `DebugWrite(ptr, len) -> len`: writes bytes (UTF-8, invalid
    /// sequences replaced) to the kernel console. Temporary, until there
    /// are handles and a console service.
    DebugWrite = 6,
}

impl Syscall {
    /// Every system call, in number order.
    pub const ALL: [Syscall; 7] = [
        Syscall::ProcessExit,
        Syscall::ThreadExit,
        Syscall::ThreadCreate,
        Syscall::Yield,
        Syscall::SleepNs,
        Syscall::ClockNowNs,
        Syscall::DebugWrite,
    ];

    /// The call with number `raw`, if any.
    pub const fn from_raw(raw: u64) -> Option<Syscall> {
        if raw < Syscall::ALL.len() as u64 {
            Some(Syscall::ALL[raw as usize])
        } else {
            None
        }
    }

    /// The number.
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

/// Error statuses (RAX after a failed call). 0 means success and is not
/// an `Error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Error {
    /// The number in RAX is not a system call.
    InvalidSyscall = 1,
    /// An argument is out of range.
    InvalidArgument = 2,
    /// A pointer argument is not mapped, readable (or writable) user
    /// memory.
    BadAddress = 3,
    /// The kernel ran out of memory.
    OutOfMemory = 4,
    /// Valid, but not available here.
    NotSupported = 5,
}

impl Error {
    /// Every error, in value order.
    pub const ALL: [Error; 5] = [
        Error::InvalidSyscall,
        Error::InvalidArgument,
        Error::BadAddress,
        Error::OutOfMemory,
        Error::NotSupported,
    ];

    /// The error with status `raw` (`None` for 0 and unknown values).
    pub const fn from_raw(raw: u64) -> Option<Error> {
        if raw >= 1 && raw <= Error::ALL.len() as u64 {
            Some(Error::ALL[raw as usize - 1])
        } else {
            None
        }
    }

    /// The status value.
    pub const fn raw(self) -> u64 {
        self as u64
    }

    /// A short name for messages.
    pub const fn name(self) -> &'static str {
        match self {
            Error::InvalidSyscall => "invalid system call",
            Error::InvalidArgument => "invalid argument",
            Error::BadAddress => "bad address",
            Error::OutOfMemory => "out of memory",
            Error::NotSupported => "not supported",
        }
    }
}

/// The status meaning success.
pub const OK: u64 = 0;

/// Turns the (RAX, RDX) pair a call returned into a `Result`. An unknown
/// non-zero status counts as `InvalidSyscall`.
pub const fn decode(status: u64, value: u64) -> Result<u64, Error> {
    if status == OK {
        Ok(value)
    } else {
        match Error::from_raw(status) {
            Some(e) => Err(e),
            None => Err(Error::InvalidSyscall),
        }
    }
}

/// Turns a kernel-side result into the (RAX, RDX) pair.
pub const fn encode(result: Result<u64, Error>) -> (u64, u64) {
    match result {
        Ok(value) => (OK, value),
        Err(e) => (e.raw(), 0),
    }
}

/// The layout of a user address space.
pub mod layout {
    /// End of the user half (exclusive): the lowest non-canonical address.
    pub const USER_END: u64 = 0x0000_8000_0000_0000;
    /// Where programs are linked (`user/user.ld`).
    pub const PROGRAM_BASE: u64 = 0x0000_0000_0040_0000;
    /// Top of the main thread's stack (exclusive).
    pub const STACK_TOP: u64 = 0x0000_7FFF_FFF0_0000;
    /// Size of the main thread's stack, all mapped at start.
    pub const STACK_SIZE: u64 = 1 << 20;
    /// The unmapped page below the stack: running into it page faults.
    pub const STACK_GUARD: u64 = STACK_TOP - STACK_SIZE - 4096;
}

// The layout's invariants, checked at compile time.
const _: () = {
    use layout::*;
    assert!(PROGRAM_BASE.is_multiple_of(4096));
    assert!(STACK_TOP.is_multiple_of(4096) && STACK_TOP < USER_END);
    assert!(STACK_SIZE.is_multiple_of(4096));
    assert!(PROGRAM_BASE < STACK_GUARD);
};

/// Most bytes one `DebugWrite` takes.
pub const DEBUG_WRITE_MAX: u64 = 4096;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_numbers_are_fixed() {
        // The numbers are the ABI; changing one breaks every program.
        assert_eq!(Syscall::ProcessExit.raw(), 0);
        assert_eq!(Syscall::ThreadExit.raw(), 1);
        assert_eq!(Syscall::ThreadCreate.raw(), 2);
        assert_eq!(Syscall::Yield.raw(), 3);
        assert_eq!(Syscall::SleepNs.raw(), 4);
        assert_eq!(Syscall::ClockNowNs.raw(), 5);
        assert_eq!(Syscall::DebugWrite.raw(), 6);
        for (i, call) in Syscall::ALL.iter().enumerate() {
            assert_eq!(call.raw(), i as u64);
            assert_eq!(Syscall::from_raw(i as u64), Some(*call));
        }
        assert_eq!(Syscall::from_raw(7), None);
        assert_eq!(Syscall::from_raw(u64::MAX), None);
    }

    #[test]
    fn error_values_are_fixed() {
        assert_eq!(Error::InvalidSyscall.raw(), 1);
        assert_eq!(Error::InvalidArgument.raw(), 2);
        assert_eq!(Error::BadAddress.raw(), 3);
        assert_eq!(Error::OutOfMemory.raw(), 4);
        assert_eq!(Error::NotSupported.raw(), 5);
        for e in Error::ALL {
            assert_eq!(Error::from_raw(e.raw()), Some(e));
            assert!(!e.name().is_empty());
        }
        assert_eq!(Error::from_raw(0), None);
        assert_eq!(Error::from_raw(6), None);
    }

    #[test]
    fn encode_decode_round_trip() {
        assert_eq!(encode(Ok(42)), (0, 42));
        assert_eq!(encode(Err(Error::BadAddress)), (3, 0));
        assert_eq!(decode(0, 42), Ok(42));
        assert_eq!(decode(3, 0), Err(Error::BadAddress));
        assert_eq!(decode(999, 0), Err(Error::InvalidSyscall));
        for e in Error::ALL {
            let (status, value) = encode(Err(e));
            assert_eq!(decode(status, value), Err(e));
        }
    }

    #[test]
    fn layout_values() {
        assert_eq!(layout::STACK_GUARD, 0x7FFF_FFDF_F000);
        assert_eq!(layout::STACK_TOP - layout::STACK_SIZE, 0x7FFF_FFE0_0000);
    }
}
