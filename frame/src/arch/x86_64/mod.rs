// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! x86-64 support.

pub(crate) mod apic;
pub(crate) mod exceptions;
pub(crate) mod gdt;
pub(crate) mod i8042;
pub(crate) mod idt;
pub(crate) mod interrupts;
pub(crate) mod io;
pub(crate) mod ioapic;
pub(crate) mod msr;
pub(crate) mod pic;

use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Loads STELA's GDT, TSS and IDT. Only the first call does anything.
pub(crate) fn init() {
    if INITIALIZED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: the flag above makes this the only call; it runs on the boot
    // processor alone with interrupts disabled (the loader executed `cli`).
    unsafe {
        gdt::init();
        idt::init(&exceptions::stubs());
    }
}

/// Whether [`init`] has run.
pub(crate) fn initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

/// Raises a breakpoint exception (`int3`), which the handler counts and
/// returns from.
pub(crate) fn breakpoint() {
    assert!(initialized(), "breakpoint() before frame::init()");
    // SAFETY: STELA's IDT is loaded (checked above) and its vector 3
    // handler saves and restores all registers and returns.
    unsafe { asm!("int3", options(nomem)) };
}

/// Number of breakpoints handled so far.
pub(crate) fn breakpoint_count() -> u64 {
    exceptions::breakpoint_count()
}

/// `sti; hlt`: enables interrupts and sleeps until one arrives.
pub(crate) fn halt_until_interrupt() {
    // SAFETY: every vector has a gate once `frame::init` has run (the
    // public wrapper is only reachable after it in practice; before, no
    // interrupt source is unmasked). `hlt` only stops this processor until
    // the next interrupt.
    unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
}

/// Disables maskable interrupts and halts forever.
pub(crate) fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli` and `hlt` only stop this processor; they touch no
        // memory. With interrupts off, `hlt` waits for an NMI at most, and the
        // loop halts again afterwards.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
