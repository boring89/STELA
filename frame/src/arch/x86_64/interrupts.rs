// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The interrupt flag RFLAGS.IF (bit 9): `cli`, `sti` and reading RFLAGS
//! (Intel SDM Vol. 1 3.4.3.3 "System Flags and IOPL Field"; Vol. 2A "CLI",
//! Vol. 2B "STI", "PUSHF/PUSHFQ").

use core::arch::asm;

/// RFLAGS.IF.
const IF: u64 = 1 << 9;

/// Whether maskable interrupts are enabled.
pub(crate) fn enabled() -> bool {
    let rflags: u64;
    // SAFETY: pushfq/pop only read RFLAGS through the stack slot they use.
    unsafe { asm!("pushfq", "pop {}", out(reg) rflags, options(nomem, preserves_flags)) };
    rflags & IF != 0
}

/// Disables maskable interrupts and returns whether they were enabled.
pub(crate) fn save_and_disable() -> bool {
    let was = enabled();
    // SAFETY: `cli` only clears IF; it touches no memory. Not marking it
    // `nomem` keeps the compiler from moving memory accesses across it.
    unsafe { asm!("cli", options(nostack, preserves_flags)) };
    was
}

/// Enables maskable interrupts.
pub(crate) fn enable() {
    // SAFETY: callers either restore a state that was already enabled, or
    // are `frame::init` after the IDT has a gate for all 256 vectors. Not
    // `nomem`, so memory accesses of the critical section that just ended
    // are not moved after it.
    unsafe { asm!("sti", options(nostack, preserves_flags)) };
}
