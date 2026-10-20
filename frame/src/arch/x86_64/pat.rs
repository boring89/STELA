// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The page attribute table (PAT, Intel SDM Vol. 3A 13.12 "Page Attribute
//! Table (PAT)"): which memory type a page gets. A 4 KiB PTE selects one of
//! eight IA32_PAT entries with its PAT (bit 7), PCD (bit 4) and PWT (bit 3)
//! bits:
//!
//! ```text
//!  entry  PAT PCD PWT   power-on   STELA
//!  0      0   0   0     WB (06)    WB
//!  1      0   0   1     WT (04)    WT
//!  2      0   1   0     UC- (07)   UC-
//!  3      0   1   1     UC (00)    UC     (map_device: PCD | PWT)
//!  4      1   0   0     WB         WC (01) ← framebuffers
//!  5..7                 WT UC- UC  unchanged
//! ```
//!
//! Write-combining collects writes in buffers and sends them in bursts:
//! much faster for a framebuffer than uncached, without the cost of
//! write-back caching device memory. Entry 4 is unused before this, so
//! changing it affects no existing mapping.

use super::msr;
use core::arch::x86_64::__cpuid;
use core::sync::atomic::{AtomicBool, Ordering};

/// IA32_PAT (Vol. 4 Table 2-2).
const IA32_PAT: u32 = 0x277;
/// Memory type encoding: write-combining.
const WC: u64 = 0x01;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Sets PAT entry 4 to write-combining, if the processor has a PAT
/// (CPUID.01H:EDX[16]; every x86-64 processor does). Called once by
/// `frame::init` before anything is mapped with entry 4.
pub(crate) fn init() {
    if __cpuid(1).edx & (1 << 16) == 0 {
        return;
    }
    // SAFETY: the PAT exists (checked above); entry 4 is selected by no
    // page yet (STELA never set the PAT bit before), so changing its type
    // cannot alias a mapping with two memory types.
    unsafe {
        let pat = msr::read(IA32_PAT);
        msr::write(IA32_PAT, (pat & !(0xFF << 32)) | (WC << 32));
    }
    ENABLED.store(true, Ordering::Release);
}

/// Gives an application processor the same PAT as the boot processor
/// (the PAT must be the same on every processor, Vol. 3A 11.12.4).
pub(crate) fn init_ap() {
    if !wc_enabled() {
        return;
    }
    // SAFETY: as in `init`; this processor has mapped nothing with entry 4.
    unsafe {
        let pat = msr::read(IA32_PAT);
        msr::write(IA32_PAT, (pat & !(0xFF << 32)) | (WC << 32));
    }
}

/// Whether PAT entry 4 is write-combining.
pub(crate) fn wc_enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// PTE bit 7 of a 4 KiB page: with PCD and PWT clear, PAT entry 4.
pub(crate) const PTE_PAT: u64 = 1 << 7;
