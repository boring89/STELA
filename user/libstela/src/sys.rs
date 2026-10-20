// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw system calls (see the `abi` crate for the convention): the number
//! in RAX, arguments in RDI, RSI, RDX; the status comes back in RAX and
//! the value in RDX; RCX and R11 are clobbered by the processor.

use abi::{Error, Syscall};
use core::arch::asm;

/// A call without arguments.
pub fn syscall0(call: Syscall) -> Result<u64, Error> {
    syscall3(call, 0, 0, 0)
}

/// A call with one argument.
pub fn syscall1(call: Syscall, a0: u64) -> Result<u64, Error> {
    syscall3(call, a0, 0, 0)
}

/// A call with two arguments.
pub fn syscall2(call: Syscall, a0: u64, a1: u64) -> Result<u64, Error> {
    syscall3(call, a0, a1, 0)
}

/// A call with three arguments.
pub fn syscall3(call: Syscall, a0: u64, a1: u64, a2: u64) -> Result<u64, Error> {
    let status: u64;
    let value: u64;
    // SAFETY: `syscall` enters the STELA kernel, which preserves every
    // register except RAX, RDX (results), RCX and R11 (declared clobbered)
    // and only reads user memory the arguments point to after checking it.
    // A call can switch threads or end the process, which is invisible to
    // this code.
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") call.raw() => status,
            in("rdi") a0,
            in("rsi") a1,
            inlateout("rdx") a2 => value,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        )
    };
    abi::decode(status, value)
}
