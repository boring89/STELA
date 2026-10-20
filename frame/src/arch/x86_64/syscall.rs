// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `syscall` entry point and the MSRs that set it up (Intel SDM Vol. 3A
//! 5.8.8 "Fast System Calls in 64-Bit Mode"; Vol. 2B "SYSCALL", "SYSRET",
//! "SWAPGS"; MSR numbers from Vol. 4 Table 2-2).
//!
//! `syscall` loads RIP from IA32_LSTAR, saves the return address in RCX and
//! RFLAGS in R11, clears the RFLAGS bits set in IA32_FMASK and switches CS
//! and SS to the kernel's; it does not touch RSP. So the entry stub:
//!
//! ```text
//!  swapgs                         GS base = this processor's PerCpu
//!  gs:[user_rsp_scratch] = rsp    park the user stack pointer
//!  rsp = gs:[kernel_rsp]          the running thread's kernel stack
//!  push user rsp, rcx (rip), r11 (rflags), rax, rdi, rsi, rdx, r10, r8, r9
//!  call syscall_dispatch(&mut SyscallFrame)
//!  cli, pop them back (rax and rdx now hold the results)
//!  rsp = user rsp, swapgs, sysretq
//! ```
//!
//! The callee-saved registers (rbx, rbp, r12-r15) are preserved by the Rust
//! code itself; the other argument registers are restored from the frame,
//! so no kernel value reaches user mode.

use super::msr;
use crate::cpu::{OFFSET_KERNEL_RSP, OFFSET_USER_RSP};
use core::arch::global_asm;

/// IA32_EFER, bit 0 = SCE (system-call enable).
const IA32_EFER: u32 = 0xC000_0080;
const EFER_SCE: u64 = 1 << 0;
/// IA32_STAR: [47:32] = kernel CS for `syscall` (SS = +8); [63:48] = base
/// for `sysret` (SS = base + 8, 64-bit CS = base + 16, RPL forced to 3).
const IA32_STAR: u32 = 0xC000_0081;
/// IA32_LSTAR: the 64-bit entry point.
const IA32_LSTAR: u32 = 0xC000_0082;
/// IA32_FMASK: RFLAGS bits cleared on entry.
const IA32_FMASK: u32 = 0xC000_0084;
/// IA32_KERNEL_GS_BASE: the base `swapgs` exchanges with GS's.
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

/// RFLAGS.TF (bit 8), IF (bit 9), DF (bit 10): no single-stepping, no
/// interrupts until the kernel stack is loaded, string ops forwards.
const FMASK: u64 = (1 << 8) | (1 << 9) | (1 << 10);

global_asm!(
    r#"
.section .text.stela_syscall, "ax"
.global stela_syscall_entry
stela_syscall_entry:
    swapgs
    mov gs:[{user_rsp}], rsp
    mov rsp, gs:[{kernel_rsp}]
    push qword ptr gs:[{user_rsp}]
    push rcx
    push r11
    push rax
    push rdi
    push rsi
    push rdx
    push r10
    push r8
    push r9
    mov rdi, rsp
    cld
    call {dispatch}
    cli
    pop r9
    pop r8
    pop r10
    pop rdx
    pop rsi
    pop rdi
    pop rax
    pop r11
    pop rcx
    pop rsp
    swapgs
    sysretq
.text
"#,
    user_rsp = const OFFSET_USER_RSP,
    kernel_rsp = const OFFSET_KERNEL_RSP,
    dispatch = sym crate::syscall::dispatch,
);

unsafe extern "C" {
    fn stela_syscall_entry();
}

/// Enables `syscall`/`sysret` and points them at the entry stub. Called
/// once per processor, after the GDT (its selector layout is what STAR
/// describes) and the per-CPU area exist.
pub(crate) fn init() {
    let star =
        (u64::from(super::gdt::KERNEL_DATA) << 48) | (u64::from(super::gdt::KERNEL_CODE) << 32);
    // SAFETY: these MSRs exist on every x86-64 processor. STAR matches the
    // GDT (kernel code 0x08, data 0x10, user data 0x18, user code 0x20);
    // LSTAR is the entry stub above, which switches to a kernel stack
    // before using any; KERNEL_GS_BASE = 0 is the user's GS base until the
    // first `swapgs`.
    unsafe {
        msr::write(IA32_STAR, star);
        msr::write(IA32_LSTAR, stela_syscall_entry as *const () as u64);
        msr::write(IA32_FMASK, FMASK);
        msr::write(IA32_KERNEL_GS_BASE, 0);
        msr::write(IA32_EFER, msr::read(IA32_EFER) | EFER_SCE);
    }
}
