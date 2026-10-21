// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The context switch, the first entry of a new thread, and the FPU/SSE
//! state.
//!
//! A suspended thread's registers live on its own kernel stack: the switch
//! pushes the callee-saved registers (System V AMD64 ABI: rbx, rbp,
//! r12-r15; everything else the compiler already assumes clobbered by the
//! call), stores rsp in the old thread's context, loads the new thread's
//! rsp, pops its registers and returns into it.
//!
//! ```text
//!  suspended stack (low address first)
//!  saved rsp ─► r15 r14 r13 r12 rbx rbp  return address
//!               └──────── popped ───────┘ └─ `ret` continues here
//! ```
//!
//! A new thread's stack is built to look the same, with r12 = entry, r13 =
//! argument and the return address = `stela_thread_trampoline`.
//!
//! FPU/SSE: FXSAVE/FXRSTOR with a 512-byte, 16-byte-aligned area (Intel SDM
//! Vol. 1 10.5 "FXSAVE and FXRSTOR Instructions"; Vol. 2A "FXSAVE").

use core::arch::{asm, global_asm};

global_asm!(
    r#"
.section .text.stela_context, "ax"
.global stela_switch_context
stela_switch_context:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp
    mov rsp, rsi
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret

.global stela_thread_trampoline
stela_thread_trampoline:
    xor ebp, ebp
    mov rdi, r12
    mov rsi, r13
    and rsp, -16
    call {start}
    ud2
.text
"#,
    start = sym crate::task::thread_start,
);

unsafe extern "C" {
    fn stela_switch_context(save_rsp: *mut u64, load_rsp: u64);
    fn stela_thread_trampoline();
}

/// The address a new thread's first switch returns to.
pub(crate) fn trampoline() -> u64 {
    stela_thread_trampoline as *const () as u64
}

/// Saves the callee-saved registers and rsp into `*save_rsp`, then
/// continues the context whose stack pointer is `load_rsp`. Returns when
/// some later switch loads the saved rsp again.
///
/// # Safety
///
/// `save_rsp` must be valid for writes until the switch returns;
/// `load_rsp` must be the saved stack pointer of a suspended context (or a
/// new stack built as described in the module documentation) whose stack
/// stays mapped while it runs. Interrupts must be disabled.
pub(crate) unsafe fn switch(save_rsp: *mut u64, load_rsp: u64) {
    // SAFETY: the caller's guarantee; the assembly follows the C calling
    // convention and preserves the callee-saved registers across the
    // switch (the resumed context restores its own).
    unsafe { stela_switch_context(save_rsp, load_rsp) };
}

/// A 512-byte FXSAVE area.
#[repr(C, align(16))]
pub(crate) struct FxArea(pub(crate) [u8; 512]);

impl FxArea {
    /// The state after FNINIT with SSE exceptions masked: FCW = 0x037F
    /// (offset 0), MXCSR = 0x1F80 (offset 24) (Vol. 1 8.1.5 "x87 FPU
    /// Control Word", 10.2.3 "MXCSR Control and Status Register"; Table
    /// 10-2 "Format of an FXSAVE Area").
    pub(crate) const fn initial() -> FxArea {
        let mut bytes = [0; 512];
        bytes[0] = 0x7F;
        bytes[1] = 0x03;
        bytes[24] = 0x80;
        bytes[25] = 0x1F;
        FxArea(bytes)
    }
}

/// Saves the x87/SSE state into `area`.
///
/// # Safety
///
/// `area` must be valid for writes and not referenced elsewhere.
pub(crate) unsafe fn fxsave(area: *mut FxArea) {
    // SAFETY: CR4.OSFXSR is set (`enable_sse`); the area is 512 bytes,
    // 16-byte aligned, and exclusively ours (caller).
    unsafe { asm!("fxsave64 [{}]", in(reg) area, options(nostack, preserves_flags)) };
}

/// Loads the x87/SSE state from `area`.
///
/// # Safety
///
/// `area` must hold a state written by FXSAVE or [`FxArea::initial`]
/// (reserved MXCSR bits clear, otherwise #GP).
pub(crate) unsafe fn fxrstor(area: *const FxArea) {
    // SAFETY: see `fxsave`; the contents are valid (caller).
    unsafe { asm!("fxrstor64 [{}]", in(reg) area, options(nostack, preserves_flags, readonly)) };
}

/// CR4.OSFXSR (bit 9) and CR4.OSXMMEXCPT (bit 10), CR0.MP (bit 1) set and
/// CR0.EM (bit 2), CR0.TS (bit 3) clear: FXSAVE/FXRSTOR and SSE work, SIMD
/// exceptions are reported as #XM (Vol. 3A 2.5 "Control Registers", 13.1.3
/// "Initialization of the SSE Extensions"). UEFI already runs with SSE on;
/// this makes it explicit.
pub(crate) fn enable_sse() {
    // SAFETY: these bits only enable instructions; the kernel itself is
    // compiled without SSE, so nothing changes for running code.
    unsafe {
        asm!(
            "mov {tmp}, cr0",
            "and {tmp}, ~0xC",
            "or {tmp}, 0x2",
            "mov cr0, {tmp}",
            "mov {tmp}, cr4",
            "or {tmp}, 0x600",
            "mov cr4, {tmp}",
            tmp = out(reg) _,
            options(nostack, preserves_flags)
        )
    };
}

/// RFLAGS for user mode: IF (bit 9) and the always-one bit 1.
const USER_RFLAGS: u64 = 0x202;

/// Enters ring 3 at `entry` with stack pointer `rsp` and `rdi` = `arg`,
/// through `iretq` (Vol. 3A 6.14.3 "IRET in IA-32e Mode"). Every other
/// general-purpose register is cleared, so no kernel value leaks. GS is
/// swapped first: the user runs with the (zero) user GS base and the
/// per-CPU base waits in IA32_KERNEL_GS_BASE for the next kernel entry.
///
/// # Safety
///
/// The processor's TSS RSP0 and per-CPU `kernel_rsp` must be the top of
/// the running thread's own kernel stack, whose contents below are no
/// longer needed (the next ring-3 interrupt starts there).
pub(crate) unsafe fn enter_user(entry: u64, rsp: u64, arg: u64) -> ! {
    // SAFETY: the caller's guarantee for the stacks; the selectors are the
    // GDT's user segments; interrupts are off from `cli` until `iretq`
    // loads RFLAGS, so nothing runs between `swapgs` and ring 3.
    unsafe {
        asm!(
            "cli",
            "swapgs",
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "xor eax, eax",
            "xor ebx, ebx",
            "xor ecx, ecx",
            "xor edx, edx",
            "xor esi, esi",
            "xor ebp, ebp",
            "xor r8d, r8d",
            "xor r9d, r9d",
            "xor r10d, r10d",
            "xor r11d, r11d",
            "xor r12d, r12d",
            "xor r13d, r13d",
            "xor r14d, r14d",
            "xor r15d, r15d",
            "iretq",
            ss = const super::gdt::USER_DATA as u64,
            cs = const super::gdt::USER_CODE as u64,
            rflags = const USER_RFLAGS,
            rsp = in(reg) rsp,
            rip = in(reg) entry,
            in("rdi") arg,
            options(noreturn)
        )
    }
}

/// Enters ring 3 with every register from `regs` (offsets of
/// `trap::UserRegs`: rax 0, rbx 8, rcx 16, rdx 24, rsi 32, rdi 40, rbp 48,
/// rsp 56, r8 64 .. r15 120, rip 128, rflags 136), through `iretq`.
///
/// # Safety
///
/// As [`enter_user`]; `regs` must be sanitized (RIP, RSP in the user half,
/// RFLAGS with IF set and no privileged bits) and lie above the current
/// stack pointer.
pub(crate) unsafe fn enter_user_regs(regs: &crate::trap::UserRegs) -> ! {
    // SAFETY: the caller's guarantee; rax holds the pointer until the last
    // load, which replaces it.
    unsafe {
        asm!(
            "cli",
            "swapgs",
            "push {ss}",
            "push qword ptr [rax + 56]",
            "push qword ptr [rax + 136]",
            "push {cs}",
            "push qword ptr [rax + 128]",
            "mov rbx, [rax + 8]",
            "mov rcx, [rax + 16]",
            "mov rdx, [rax + 24]",
            "mov rsi, [rax + 32]",
            "mov rdi, [rax + 40]",
            "mov rbp, [rax + 48]",
            "mov r8, [rax + 64]",
            "mov r9, [rax + 72]",
            "mov r10, [rax + 80]",
            "mov r11, [rax + 88]",
            "mov r12, [rax + 96]",
            "mov r13, [rax + 104]",
            "mov r14, [rax + 112]",
            "mov r15, [rax + 120]",
            "mov rax, [rax]",
            "iretq",
            ss = const super::gdt::USER_DATA as u64,
            cs = const super::gdt::USER_CODE as u64,
            in("rax") regs as *const crate::trap::UserRegs,
            options(noreturn)
        )
    }
}

/// The MXCSR bits FXRSTOR accepts: the MXCSR_MASK field of an FXSAVE
/// image (offset 28), or 0xFFBF when it reads 0 (Vol. 1 11.6.6 "Guidelines
/// for Writing to the MXCSR Register").
pub(crate) fn mxcsr_mask() -> u32 {
    static MASK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let known = MASK.load(core::sync::atomic::Ordering::Relaxed);
    if known != 0 {
        return known;
    }
    let mut area = FxArea([0; 512]);
    // SAFETY: a local, aligned area; FXSAVE only writes it.
    unsafe { fxsave(&mut area) };
    let mut mask = u32::from_le_bytes([area.0[28], area.0[29], area.0[30], area.0[31]]);
    if mask == 0 {
        mask = 0xFFBF;
    }
    MASK.store(mask, core::sync::atomic::Ordering::Relaxed);
    mask
}
