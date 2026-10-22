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
pub(crate) mod pat;
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

/// `sti; hlt` (or `monitor; sti; mwait` with the C1 hint, P14 K19):
/// enables interrupts and sleeps until one arrives.
pub(crate) fn halt_until_interrupt() {
    assert!(initialized(), "halt_until_interrupt() before frame::init()");
    if MWAIT.load(Ordering::Relaxed) {
        let line = IDLE_LINE.0.as_ptr() as u64;
        // SAFETY: `monitor` only arms this processor's address monitor on
        // IDLE_LINE, a static of ordinary write-back memory (Intel SDM Vol.
        // 2B, MONITOR: RAX the address, ECX extensions 0, EDX hints 0).
        // Then as `sti; hlt`: every vector has a gate, and `mwait` right
        // after `sti` is in its interrupt shadow, so no interrupt slips in
        // between; with EAX 0 (C1, sub-state 0) and ECX 0 it wakes on an
        // interrupt (or a write to the line, nobody writes it).
        unsafe {
            asm!("monitor", in("rax") line, in("ecx") 0, in("edx") 0, options(nostack, preserves_flags));
            asm!("sti", "mwait", in("eax") 0, in("ecx") 0, options(nomem, nostack));
        }
        return;
    }
    // SAFETY: the IDT has a gate for all 256 vectors (checked above), so
    // any interrupt that arrives is handled. `hlt` only stops this
    // processor until the next interrupt.
    unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
}

/// Whether the idle loop uses `mwait` (P14 K19).
static MWAIT: AtomicBool = AtomicBool::new(false);

/// The cache line `monitor` watches while idle (its own 64 bytes).
#[repr(C, align(64))]
struct IdleLine([u64; 8]);
static IDLE_LINE: IdleLine = IdleLine([0; 8]);

/// Uses `mwait` (C1) instead of `hlt` when the processor has it
/// ([`crate::cpuid::mwait_c1`]), from the boot option `idle=`: `hlt`
/// keeps `hlt`; `mwait` uses it whenever CPUID has MONITOR/MWAIT, even
/// without a C1 sub-state in leaf 05H (QEMU's TCG reports none, so its
/// test needs this); anything else decides by CPUID. All processors are
/// taken to be alike (the boot processor decides).
pub fn choose_idle(option: Option<&[u8]>) {
    let max = core::arch::x86_64::__cpuid(0).eax;
    let ecx1 = core::arch::x86_64::__cpuid(1).ecx;
    let leaf5 = if max >= 5 {
        let r = core::arch::x86_64::__cpuid(5);
        [r.eax, r.ebx, r.ecx, r.edx]
    } else {
        [0; 4]
    };
    let use_mwait = match option {
        Some(b"hlt") => false,
        Some(b"mwait") => ecx1 & (1 << 3) != 0,
        _ => crate::cpuid::mwait_c1(max, ecx1, leaf5),
    };
    MWAIT.store(use_mwait, Ordering::Relaxed);
}

/// "mwait" or "hlt": what the idle loop uses.
pub fn idle_method() -> &'static str {
    if MWAIT.load(Ordering::Relaxed) {
        "mwait"
    } else {
        "hlt"
    }
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
