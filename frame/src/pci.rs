// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! PCI configuration space through the legacy I/O ports (configuration
//! mechanism #1, PCI Local Bus Specification 3.0 section 3.2.2.3.2): write
//! the address of a 32-bit register to CONFIG_ADDRESS (0xCF8), then read or
//! write CONFIG_DATA (0xCFC).
//!
//! ```text
//!  CONFIG_ADDRESS  bit 31 enable | 23:16 bus | 15:11 device | 10:8 function | 7:2 register
//! ```
//!
//! This reaches only the first 256 bytes of each function; machines with
//! an ACPI MCFG use the memory-mapped ECAM instead (`acpi::mcfg`, mapped
//! with `mm::mmio`). The pair of ports is owned here and used under a lock,
//! so an address and its data access are never split.
//!
//! Configuration writes can start devices (bus mastering, DMA); like MMIO
//! writes, they are trusted kernel code's responsibility.

use crate::arch::x86_64::io::{inl, outl};
use crate::sync::SpinLock;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

static PORTS: SpinLock<()> = SpinLock::new(());

fn address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    0x8000_0000
        | u32::from(bus) << 16
        | u32::from(device & 0x1F) << 11
        | u32::from(function & 7) << 8
        | u32::from(offset & 0xFC)
}

/// Reads the 32-bit register at `offset` (rounded down to 4) of a
/// function.
pub fn legacy_read32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let _guard = PORTS.lock();
    // SAFETY: 0xCF8/0xCFC are the PCI configuration ports, used only here
    // and under the lock; selecting and reading a configuration register
    // has no effect on memory.
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, device, function, offset));
        inl(CONFIG_DATA)
    }
}

/// Writes the 32-bit register at `offset` (rounded down to 4) of a
/// function.
pub fn legacy_write32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let _guard = PORTS.lock();
    // SAFETY: as in `legacy_read32`; what the write makes the device do is
    // the caller's (trusted kernel code's) responsibility, as with MMIO.
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, device, function, offset));
        outl(CONFIG_DATA, value);
    }
}
