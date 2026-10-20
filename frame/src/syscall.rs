// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! System calls: the registers a program passed with `syscall`, and the
//! hook through which the kernel handles them. The entry stub is in
//! `arch/x86_64/syscall.rs`; the meaning of the numbers is the `abi` crate
//! and the kernel's business.

use crate::sync::Once;
use crate::trap::TrapFrame;

/// The user registers saved by the entry stub, lowest address first.
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    /// Argument 6.
    pub r9: u64,
    /// Argument 5.
    pub r8: u64,
    /// Argument 4.
    pub r10: u64,
    /// Argument 3 on entry; the result value on return.
    pub rdx: u64,
    /// Argument 2.
    pub rsi: u64,
    /// Argument 1.
    pub rdi: u64,
    /// The number on entry; the status on return.
    pub rax: u64,
    /// User RFLAGS (from R11); restored by `sysret`.
    rflags: u64,
    /// User return address (from RCX).
    rip: u64,
    /// User stack pointer.
    rsp: u64,
}

const _: () = assert!(core::mem::size_of::<SyscallFrame>() == 80);

impl SyscallFrame {
    /// The system-call number (RAX).
    pub fn number(&self) -> u64 {
        self.rax
    }

    /// The six arguments: RDI, RSI, RDX, R10, R8, R9.
    pub fn args(&self) -> [u64; 6] {
        [self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9]
    }

    /// Sets the status (RAX) and value (RDX) the program gets back.
    pub fn set_return(&mut self, status: u64, value: u64) {
        self.rax = status;
        self.rdx = value;
    }

    /// Where the program continues.
    pub fn rip(&self) -> u64 {
        self.rip
    }

    /// The program's stack pointer.
    pub fn rsp(&self) -> u64 {
        self.rsp
    }
}

/// The kernel's system-call handler.
pub type Handler = fn(&mut SyscallFrame);

static HANDLER: Once<Handler> = Once::new();

/// Registers the system-call handler. It runs with interrupts enabled on
/// the calling thread's kernel stack, may block or switch threads, and may
/// end the thread (never returning). Only the first registration counts.
pub fn set_handler(handler: Handler) {
    let _ = HANDLER.set(handler);
}

/// Called by the entry stub with interrupts disabled.
pub(crate) extern "C" fn dispatch(frame: &mut SyscallFrame) {
    crate::irq::enable();
    match HANDLER.get() {
        Some(handler) => handler(frame),
        None => frame.set_return(u64::MAX, 0),
    }
    // From here to `sysretq` interrupts stay off, so nothing can kill the
    // thread between this check and the return to ring 3.
    crate::arch::x86_64::interrupts::save_and_disable();
    crate::task::user_return();
    // `sysret` to a non-canonical RCX would fault in ring 0 with the user
    // stack loaded (Intel SDM Vol. 2B "SYSRET", #GP). RIP was the address
    // after the user's `syscall`, so this only happens for a `syscall` at
    // the very end of the user half: treat it as the #GP it would be.
    if frame.rip >= crate::mm::USER_END {
        let _irq = crate::sync::IrqGuard::new();
        let trap = TrapFrame {
            vector: 13,
            rip: frame.rip,
            rsp: frame.rsp,
            cs: u64::from(crate::arch::x86_64::gdt::USER_CODE),
            rflags: frame.rflags,
            ..TrapFrame::default()
        };
        crate::task::user_fault(&trap, 0);
    }
}
