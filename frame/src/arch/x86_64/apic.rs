// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The local APIC in xAPIC mode: one per processor, at the same physical
//! address on every processor (each sees its own), reached through an
//! uncached MMIO mapping (Intel SDM Vol. 3A "Advanced Programmable Interrupt
//! Controller (APIC)": "Local APIC Register Address Map", "Local Vector
//! Table", "Spurious-Interrupt Vector Register", "End-Of-Interrupt (EOI)
//! Register", "Interrupt Command Register (ICR)").
//!
//! ```text
//! offset  register                    STELA sets
//! 0x020   ID (bits 24-31)
//! 0x030   version (bits 16-23: max LVT index)
//! 0x080   TPR, task priority          0: accept every vector
//! 0x0B0   EOI                         write 0 at the end of an interrupt
//! 0x0F0   spurious vector register    0x1FF: software enable + vector 0xFF
//! 0x280   ESR, error status
//! 0x300   ICR low / 0x310 ICR high    self-IPIs
//! 0x320   LVT timer                   masked (time.rs programs it)
//! 0x330   LVT thermal, 0x340 perf     masked
//! 0x350   LVT LINT0, 0x360 LINT1      masked (the 8259 and NMI pins)
//! 0x370   LVT error                   masked
//! 0x380   timer initial count, 0x390 current count, 0x3E0 divide
//! ```

use super::msr;
use crate::acpi::tables::Madt;
use crate::mm::mmio::{self, MmioRegion};
use crate::sync::Once;
use core::arch::x86_64::__cpuid;

pub(crate) const ID: usize = 0x020;
const VERSION: usize = 0x030;
const TPR: usize = 0x080;
const EOI: usize = 0x0B0;
const SVR: usize = 0x0F0;
const ESR: usize = 0x280;
const ICR_LOW: usize = 0x300;
const ICR_HIGH: usize = 0x310;
const LVT_CMCI: usize = 0x2F0;
pub(crate) const LVT_TIMER: usize = 0x320;
const LVT_THERMAL: usize = 0x330;
const LVT_PERF: usize = 0x340;
const LVT_LINT0: usize = 0x350;
const LVT_LINT1: usize = 0x360;
const LVT_ERROR: usize = 0x370;

/// LVT bit 16: masked.
pub(crate) const LVT_MASKED: u32 = 1 << 16;
/// SVR bit 8: APIC software enable.
const SVR_ENABLE: u32 = 1 << 8;

/// IA32_APIC_BASE bit 11: APIC global enable.
const BASE_ENABLE: u64 = 1 << 11;
/// IA32_APIC_BASE bit 10: x2APIC mode.
const BASE_X2APIC: u64 = 1 << 10;
/// IA32_APIC_BASE bits 12 and up: physical base address.
const BASE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// The registers, mapped once by the boot processor.
static REGS: Once<MmioRegion> = Once::new();

fn regs() -> &'static MmioRegion {
    match REGS.get() {
        Some(regs) => regs,
        None => panic!("apic: used before frame::init()"),
    }
}

/// Reads a local APIC register.
pub(crate) fn read(register: usize) -> u32 {
    regs().read_u32(register)
}

/// Writes a local APIC register.
pub(crate) fn write(register: usize, value: u32) {
    regs().write_u32(register, value);
}

/// Turns on this processor's local APIC in xAPIC mode with every local
/// interrupt source masked, the spurious vector at `spurious` and task
/// priority 0. Returns its APIC id. Panics if the processor has no APIC.
pub(crate) fn init(madt: &Madt, spurious: u8) -> u32 {
    // CPUID.01H:EDX[9]: on-chip APIC (Vol. 2A "CPUID", Table 3-11).
    let features = __cpuid(1);
    assert!(
        features.edx & (1 << 9) != 0,
        "apic: this processor has no local APIC; STELA needs one"
    );
    // SAFETY: IA32_APIC_BASE exists on every processor with an APIC
    // (checked above).
    let base = unsafe { msr::read(msr::IA32_APIC_BASE) };
    if base & BASE_X2APIC != 0 {
        // x2APIC -> xAPIC must pass through the disabled state (Vol. 3A
        // "x2APIC State Transitions").
        // SAFETY: disabling and re-enabling the local APIC while interrupts
        // are off only changes how its registers are reached.
        unsafe {
            msr::write(msr::IA32_APIC_BASE, base & !(BASE_ENABLE | BASE_X2APIC));
            msr::write(msr::IA32_APIC_BASE, (base & !BASE_X2APIC) | BASE_ENABLE);
        }
    } else if base & BASE_ENABLE == 0 {
        // SAFETY: as above; this sets the global enable bit.
        unsafe { msr::write(msr::IA32_APIC_BASE, base | BASE_ENABLE) };
    }
    let msr_phys = base & BASE_ADDR_MASK;
    if msr_phys != madt.local_apic_address {
        crate::kprintln!(
            "apic: MADT says {:#x}, IA32_APIC_BASE says {msr_phys:#x}; using the MSR",
            madt.local_apic_address
        );
    }
    if REGS.get().is_none() {
        let regs = match mmio::map(msr_phys, 4096) {
            Ok(regs) => regs,
            Err(e) => panic!("apic: cannot map the local APIC at {msr_phys:#x}: {e:?}"),
        };
        let _ = REGS.set(regs);
    }
    // Enable first: while software-disabled, the LVT mask bits cannot be
    // changed (Vol. 3A "Spurious-Interrupt Vector Register").
    write(SVR, SVR_ENABLE | u32::from(spurious));
    let max_lvt = (read(VERSION) >> 16) & 0xFF;
    write(LVT_TIMER, LVT_MASKED);
    write(LVT_LINT0, LVT_MASKED);
    write(LVT_LINT1, LVT_MASKED);
    write(LVT_ERROR, LVT_MASKED);
    // The other LVT entries exist only if the version register says so.
    if max_lvt >= 4 {
        write(LVT_PERF, LVT_MASKED);
    }
    if max_lvt >= 5 {
        write(LVT_THERMAL, LVT_MASKED);
    }
    if max_lvt >= 6 {
        write(LVT_CMCI, LVT_MASKED);
    }
    // ESR is cleared by writing it (twice: the first write latches).
    write(ESR, 0);
    write(ESR, 0);
    write(TPR, 0);
    eoi();
    id()
}

/// This processor's local APIC id.
pub(crate) fn id() -> u32 {
    read(ID) >> 24
}

/// Signals the end of the current interrupt.
pub(crate) fn eoi() {
    write(EOI, 0);
}

/// ICR bit 12: the previous IPI has not been accepted yet.
const ICR_PENDING: u32 = 1 << 12;
/// ICR bit 14: level assert (required for fixed IPIs on older processors).
const ICR_ASSERT: u32 = 1 << 14;
/// ICR bits 18-19 = 01: destination shorthand "self".
const ICR_SELF: u32 = 1 << 18;

/// Sends interrupt `vector` (fixed delivery) to this processor.
pub(crate) fn self_ipi(vector: u8) {
    while read(ICR_LOW) & ICR_PENDING != 0 {
        core::hint::spin_loop();
    }
    write(ICR_HIGH, 0);
    write(ICR_LOW, ICR_SELF | ICR_ASSERT | u32::from(vector));
    while read(ICR_LOW) & ICR_PENDING != 0 {
        core::hint::spin_loop();
    }
}
