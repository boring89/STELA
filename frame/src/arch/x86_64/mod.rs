// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! x86-64 support.

pub(crate) mod apic;
pub(crate) mod cmos;
pub(crate) mod context;
pub(crate) mod exceptions;
pub(crate) mod gdt;
pub(crate) mod i8042;
pub(crate) mod idt;
pub(crate) mod interrupts;
pub(crate) mod io;
pub(crate) mod ioapic;
pub(crate) mod msr;
pub(crate) mod pic;
pub(crate) mod pit;
pub(crate) mod syscall;

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
    assert!(initialized(), "halt_until_interrupt() before frame::init()");
    // SAFETY: the IDT has a gate for all 256 vectors (checked above), so
    // any interrupt that arrives is handled. `hlt` only stops this
    // processor until the next interrupt.
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

/// Whether SMEP (supervisor-mode execution prevention) is on.
static SMEP: AtomicBool = AtomicBool::new(false);

/// Turns on SMEP when the processor has it (CPUID.(EAX=07H,ECX=0):EBX[7]):
/// with CR4.SMEP (bit 20) set, ring 0 cannot execute pages that ring 3 can
/// reach (Intel SDM Vol. 3A 4.6.1 "Determination of Access Rights").
pub(crate) fn enable_smep() {
    let max = core::arch::x86_64::__cpuid(0).eax;
    if max < 7 || core::arch::x86_64::__cpuid_count(7, 0).ebx & (1 << 7) == 0 {
        return;
    }
    // SAFETY: the kernel never executes user pages, so forbidding it only
    // turns a bug into a page fault.
    unsafe {
        asm!(
            "mov {tmp}, cr4",
            "bts {tmp}, 20",
            "mov cr4, {tmp}",
            tmp = out(reg) _,
            options(nostack)
        )
    };
    SMEP.store(true, Ordering::Relaxed);
}

/// Whether SMEP is enabled.
pub fn smep_enabled() -> bool {
    SMEP.load(Ordering::Relaxed)
}
