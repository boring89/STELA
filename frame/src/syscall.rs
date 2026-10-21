// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! System calls: the registers a program passed with `syscall`, and the
//! hook through which the kernel handles them. The entry stub is in
//! `arch/x86_64/syscall.rs`; the meaning of the numbers is the `abi` crate
//! and the kernel's business.

use crate::sync::Once;
use crate::trap::{BadRegs, TrapFrame, UserRegs};

/// The user registers saved by the entry stub, lowest address first.
/// Every general-purpose register is saved, so that the kernel can see and
/// replace the whole user state (signal frames, `fork`, `execve`:
/// [`SyscallFrame::regs`], [`SyscallFrame::set_regs`]).
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    /// Nonzero: return with `iretq`, restoring RCX and R11 from
    /// `user_rcx`/`user_r11` (set by [`SyscallFrame::set_regs`]); zero:
    /// `sysretq`, which loads RCX = RIP and R11 = RFLAGS.
    full: u64,
    /// Keeps the frame a multiple of 16 bytes (the stub calls into Rust
    /// right below it).
    _pad: u64,
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbp: u64,
    rbx: u64,
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
    /// R11 as the program will see it after an `iretq` return (on entry
    /// the user RFLAGS, which `syscall` put there).
    user_r11: u64,
    /// RCX likewise (on entry the return address).
    user_rcx: u64,
    /// User RFLAGS (from R11); restored by `sysret`.
    rflags: u64,
    /// User return address (from RCX).
    rip: u64,
    /// User stack pointer.
    rsp: u64,
}

const _: () = assert!(core::mem::size_of::<SyscallFrame>() == 160);

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

    /// Sets only RAX (Linux programs get their result there and expect
    /// every other register back unchanged).
    pub fn set_rax(&mut self, value: u64) {
        self.rax = value;
    }

    /// Where the program continues.
    pub fn rip(&self) -> u64 {
        self.rip
    }

    /// The program's stack pointer.
    pub fn rsp(&self) -> u64 {
        self.rsp
    }

    /// The user registers as they will be on return (RCX and R11 as the
    /// `syscall` instruction left them, unless replaced).
    pub fn regs(&self) -> UserRegs {
        UserRegs {
            rax: self.rax,
            rbx: self.rbx,
            rcx: self.user_rcx,
            rdx: self.rdx,
            rsi: self.rsi,
            rdi: self.rdi,
            rbp: self.rbp,
            rsp: self.rsp,
            r8: self.r8,
            r9: self.r9,
            r10: self.r10,
            r11: self.user_r11,
            r12: self.r12,
            r13: self.r13,
            r14: self.r14,
            r15: self.r15,
            rip: self.rip,
            rflags: self.rflags,
        }
    }

    /// Replaces every user register (after [`UserRegs::sanitized`]); the
    /// return then goes through `iretq`, so RCX and R11 are restored too.
    pub fn set_regs(&mut self, regs: &UserRegs) -> Result<(), BadRegs> {
        let r = regs.sanitized()?;
        self.rax = r.rax;
        self.rbx = r.rbx;
        self.user_rcx = r.rcx;
        self.rdx = r.rdx;
        self.rsi = r.rsi;
        self.rdi = r.rdi;
        self.rbp = r.rbp;
        self.rsp = r.rsp;
        self.r8 = r.r8;
        self.r9 = r.r9;
        self.r10 = r.r10;
        self.user_r11 = r.r11;
        self.r12 = r.r12;
        self.r13 = r.r13;
        self.r14 = r.r14;
        self.r15 = r.r15;
        self.rip = r.rip;
        self.rflags = r.rflags;
        self.full = 1;
        Ok(())
    }
}

/// The user state a thread returns to ring 3 with: a system call's frame
/// or an interrupt's (or exception's). Given to the user-return hook
/// (`task::set_user_return_hook`), which may replace the registers.
pub enum UserFrame<'a> {
    /// Returning from a system call.
    Syscall(&'a mut SyscallFrame),
    /// Returning from an interrupt or exception that came from ring 3.
    Trap(&'a mut TrapFrame),
}

impl UserFrame<'_> {
    /// The user registers.
    pub fn regs(&self) -> UserRegs {
        match self {
            UserFrame::Syscall(f) => f.regs(),
            UserFrame::Trap(f) => f.user_regs(),
        }
    }

    /// Replaces the user registers.
    pub fn set_regs(&mut self, regs: &UserRegs) -> Result<(), BadRegs> {
        match self {
            UserFrame::Syscall(f) => f.set_regs(regs),
            UserFrame::Trap(f) => f.set_user_regs(regs),
        }
    }

    /// The system call being returned from, if any.
    pub fn syscall(&mut self) -> Option<&mut SyscallFrame> {
        match self {
            UserFrame::Syscall(f) => Some(f),
            UserFrame::Trap(_) => None,
        }
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
    crate::task::user_return(&mut UserFrame::Syscall(frame));
    // `sysret` to a non-canonical RCX would fault in ring 0 with the user
    // stack loaded (Intel SDM Vol. 2B "SYSRET", #GP). RIP was the address
    // after the user's `syscall`, so this only happens for a `syscall` at
    // the very end of the user half: treat it as the #GP it would be.
    if frame.rip >= crate::mm::USER_END {
        let _irq = crate::sync::IrqGuard::new();
        let regs = frame.regs();
        let mut trap = TrapFrame {
            vector: 13,
            cs: u64::from(crate::arch::x86_64::gdt::USER_CODE),
            ss: u64::from(crate::arch::x86_64::gdt::USER_DATA),
            ..TrapFrame::default()
        };
        trap.rax = regs.rax;
        trap.rbx = regs.rbx;
        trap.rcx = regs.rcx;
        trap.rdx = regs.rdx;
        trap.rsi = regs.rsi;
        trap.rdi = regs.rdi;
        trap.rbp = regs.rbp;
        trap.rsp = regs.rsp;
        trap.r8 = regs.r8;
        trap.r9 = regs.r9;
        trap.r10 = regs.r10;
        trap.r11 = regs.r11;
        trap.r12 = regs.r12;
        trap.r13 = regs.r13;
        trap.r14 = regs.r14;
        trap.r15 = regs.r15;
        trap.rip = regs.rip;
        trap.rflags = regs.rflags;
        // Ends the thread, or redirects it (a signal handler): then the
        // new registers go back through the iretq path.
        crate::task::user_fault(&mut trap, 0);
        if frame.set_regs(&trap.user_regs()).is_err() {
            panic!("syscall: return to a non-canonical address after a fault");
        }
    }
}
