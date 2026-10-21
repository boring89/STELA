// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raw system calls (see the `abi` crate for the convention): the number
//! in RAX, arguments in RDI, RSI, RDX, R10, R8, R9; the status comes back
//! in RAX and the value in RDX; RCX and R11 are clobbered by the
//! processor.

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
    syscall6(call, [a0, a1, a2, 0, 0, 0])
}

/// A call with four arguments.
pub fn syscall4(call: Syscall, a0: u64, a1: u64, a2: u64, a3: u64) -> Result<u64, Error> {
    syscall6(call, [a0, a1, a2, a3, 0, 0])
}

/// A call with up to six arguments (RDI, RSI, RDX, R10, R8, R9).
pub fn syscall6(call: Syscall, args: [u64; 6]) -> Result<u64, Error> {
    let [a0, a1, a2, a3, a4, a5] = args;
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
            in("r10") a3,
            in("r8") a4,
            in("r9") a5,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        )
    };
    abi::decode(status, value)
}

/// Fills `buf` with random bytes from the kernel (`Random`, P12).
pub fn random(buf: &mut [u8]) -> Result<(), Error> {
    for chunk in buf.chunks_mut(abi::RANDOM_MAX as usize) {
        syscall2(
            Syscall::Random,
            chunk.as_mut_ptr() as u64,
            chunk.len() as u64,
        )?;
    }
    Ok(())
}
