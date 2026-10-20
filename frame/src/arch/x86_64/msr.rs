// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Model-specific registers: `rdmsr` and `wrmsr` (Intel SDM Vol. 2B
//! "RDMSR", "WRMSR"; register numbers from Vol. 4 "Model-Specific
//! Registers", Table 2-2 "IA-32 Architectural MSRs").

use core::arch::asm;

/// IA32_APIC_BASE: local APIC base address and enable bits.
pub(crate) const IA32_APIC_BASE: u32 = 0x1B;
/// IA32_FS_BASE: the base address of the FS segment (a Linux program's
/// thread pointer).
pub(crate) const IA32_FS_BASE: u32 = 0xC000_0100;
/// IA32_GS_BASE: the base address of the GS segment.
pub(crate) const IA32_GS_BASE: u32 = 0xC000_0101;

/// Reads an MSR.
///
/// # Safety
///
/// `msr` must exist on this processor (otherwise `rdmsr` raises #GP).
pub(crate) unsafe fn read(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: the caller guarantees the MSR exists; reading has no side
    // effects on memory.
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high,
             options(nomem, nostack, preserves_flags))
    };
    (u64::from(high) << 32) | u64::from(low)
}

/// Writes an MSR.
///
/// # Safety
///
/// `msr` must exist and `value` must be valid for it, and the change must
/// not break memory safety (for example moving GS_BASE while code relies on
/// it).
pub(crate) unsafe fn write(msr: u32, value: u64) {
    // SAFETY: the caller's guarantee.
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32,
             options(nostack, preserves_flags))
    };
}
