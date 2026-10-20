// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Entry stubs for all 256 vectors and the Rust dispatcher.
//!
//! Vectors 0-31 are exceptions; 32-255 are interrupts (device IRQs, the
//! APIC timer, IPIs, the spurious vector), whose stubs are generated with
//! `.rept` and go through the same common routine; [`trap_dispatch`] hands
//! them to `crate::irq`.
//!
//! The processor pushes SS, RSP, RFLAGS, CS, RIP and, for some vectors, an
//! error code (Intel SDM Vol. 3A 6.13 "Error Code", Table 6-1). Each stub
//! makes the frame uniform by pushing a zero where there is no error code,
//! then pushes its vector number and jumps to a common routine that saves
//! the general-purpose registers and calls [`exception_dispatch`] with a
//! pointer to the resulting [`TrapFrame`]. If the dispatcher returns, the
//! registers are restored and `iretq` resumes the interrupted code.
//!
//! Stack alignment: in 64-bit mode the processor aligns RSP to 16 bytes
//! before pushing its frame (Vol. 3A 6.14.2). 5 words + error code +
//! vector + 15 registers = 22 words = 176 bytes, a multiple of 16, so RSP is
//! 16-byte aligned at the `call`, as the System V ABI requires.

use crate::trap::{Report, TrapFrame};
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering};

global_asm!(
    r#"
.macro STELA_EXC_NOERR vec
    .p2align 4
stela_exception_\vec:
    push 0
    push \vec
    jmp stela_exception_common
.endm

.macro STELA_EXC_ERR vec
    .p2align 4
stela_exception_\vec:
    push \vec
    jmp stela_exception_common
.endm

.section .text.stela_exceptions, "ax"
    STELA_EXC_NOERR 0
    STELA_EXC_NOERR 1
    STELA_EXC_NOERR 2
    STELA_EXC_NOERR 3
    STELA_EXC_NOERR 4
    STELA_EXC_NOERR 5
    STELA_EXC_NOERR 6
    STELA_EXC_NOERR 7
    STELA_EXC_ERR   8
    STELA_EXC_NOERR 9
    STELA_EXC_ERR   10
    STELA_EXC_ERR   11
    STELA_EXC_ERR   12
    STELA_EXC_ERR   13
    STELA_EXC_ERR   14
    STELA_EXC_NOERR 15
    STELA_EXC_NOERR 16
    STELA_EXC_ERR   17
    STELA_EXC_NOERR 18
    STELA_EXC_NOERR 19
    STELA_EXC_NOERR 20
    STELA_EXC_ERR   21
    STELA_EXC_NOERR 22
    STELA_EXC_NOERR 23
    STELA_EXC_NOERR 24
    STELA_EXC_NOERR 25
    STELA_EXC_NOERR 26
    STELA_EXC_NOERR 27
    STELA_EXC_NOERR 28
    STELA_EXC_ERR   29
    STELA_EXC_ERR   30
    STELA_EXC_NOERR 31

// Interrupt stubs 32..255. The processor pushes no error code for them.
// Each stub is placed at a 16-byte boundary and is at most 12 bytes
// (push imm8 2, push imm32 5, jmp rel32 5), so stub n starts at
// stela_irq_stubs + (n - 32) * 16; stubs() checks the total size at boot.
    .p2align 4
stela_irq_stubs:
.set stela_vec, 32
.rept 224
    .p2align 4
    push 0
    push stela_vec
    jmp stela_exception_common
.set stela_vec, stela_vec + 1
.endr
    .p2align 4
stela_irq_stubs_end:

// Coming from ring 3 (the saved CS has RPL 3), GS holds the user's base:
// swap in the per-CPU one (IA32_KERNEL_GS_BASE) before any kernel code
// runs, and swap back before returning (Vol. 2B "SWAPGS").
stela_exception_common:
    test qword ptr [rsp + 24], 3
    jz 1f
    swapgs
1:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    cld
    call {dispatch}
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq

.section .rodata.stela_exceptions, "a"
    .p2align 3
stela_exception_stubs:
    .quad stela_exception_0, stela_exception_1, stela_exception_2, stela_exception_3
    .quad stela_exception_4, stela_exception_5, stela_exception_6, stela_exception_7
    .quad stela_exception_8, stela_exception_9, stela_exception_10, stela_exception_11
    .quad stela_exception_12, stela_exception_13, stela_exception_14, stela_exception_15
    .quad stela_exception_16, stela_exception_17, stela_exception_18, stela_exception_19
    .quad stela_exception_20, stela_exception_21, stela_exception_22, stela_exception_23
    .quad stela_exception_24, stela_exception_25, stela_exception_26, stela_exception_27
    .quad stela_exception_28, stela_exception_29, stela_exception_30, stela_exception_31
.text
"#,
    dispatch = sym trap_dispatch,
);

/// Size reserved for each interrupt stub.
const IRQ_STUB_SIZE: u64 = 16;

/// The addresses of the 256 stubs, in vector order.
pub(crate) fn stubs() -> [u64; 256] {
    unsafe extern "C" {
        static stela_exception_stubs: [u64; 32];
        static stela_irq_stubs: u8;
        static stela_irq_stubs_end: u8;
    }
    // SAFETY: the table is defined above in read-only data, fully
    // initialised by the assembler and never written.
    let exceptions = unsafe { &*core::ptr::addr_of!(stela_exception_stubs) };
    let first = core::ptr::addr_of!(stela_irq_stubs) as u64;
    let end = core::ptr::addr_of!(stela_irq_stubs_end) as u64;
    // Every stub must have kept to its 16 bytes (the assembler only pads).
    assert_eq!(
        end - first,
        224 * IRQ_STUB_SIZE,
        "irq stubs are not 16 bytes each"
    );
    let mut stubs = [0; 256];
    stubs[..32].copy_from_slice(exceptions);
    for (i, stub) in stubs[32..].iter_mut().enumerate() {
        *stub = first + i as u64 * IRQ_STUB_SIZE;
    }
    stubs
}

/// Number of breakpoint (`int3`) exceptions handled so far.
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);

/// How many breakpoints have been handled.
pub(crate) fn breakpoint_count() -> u64 {
    BREAKPOINTS.load(Ordering::Relaxed)
}

/// Reads CR2, the linear address that caused the last page fault
/// (Intel SDM Vol. 3A 2.5 "Control Registers").
fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: reading CR2 has no side effects.
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Called by the common stub with the saved registers.
///
/// Interrupts (vector 32 and up) go to `crate::irq`. An exception in ring 3
/// goes to the kernel's user-fault handler (`crate::task`), which ends the
/// process. In ring 0 a breakpoint is counted and execution resumes after
/// the `int3`; every other exception is fatal: it panics with a register
/// dump.
extern "C" fn trap_dispatch(frame: &mut TrapFrame) {
    if frame.vector >= 32 {
        crate::irq::dispatch(frame);
        return;
    }
    let cr2 = read_cr2();
    if frame.cs & 3 == 3 {
        crate::task::user_fault(frame, cr2);
    }
    if frame.vector == 3 {
        BREAKPOINTS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    panic!("{}", Report { frame, cr2 });
}
