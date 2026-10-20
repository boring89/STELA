// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! ACPI: finding the MADT, which lists the processors, the I/O APICs and
//! how the ISA interrupts are wired.
//!
//! The firmware leaves its tables in memory; the loader passes the physical
//! address of the RSDP (`BootInfo::rsdp_phys`). The chain is
//!
//! ```text
//! RSDP ──► XSDT (or RSDT on ACPI 1.0) ──► [FACP, APIC, HPET, MCFG, ...]
//!                                              └─ "APIC" = the MADT
//! ```
//!
//! The tables are read through the HHDM (they are in ACPI reclaimable or
//! NVS memory, which it covers). Only the MADT is parsed; the logic is in
//! [`tables`] and tested on the host.

pub mod tables;

#[cfg(target_os = "none")]
pub use target::{init, madt};

#[cfg(target_os = "none")]
mod target {
    use super::tables::{self, AcpiError, Madt};
    use crate::mm::frames::overlaps;
    use crate::sync::SpinLock;
    use alloc::boxed::Box;
    use bootinfo::{BootInfo, HHDM_BASE, MemoryKind};

    /// Largest table accepted; real MADTs and XSDTs are a few KiB.
    const MAX_TABLE: usize = 1 << 20;

    static MADT: SpinLock<Option<&'static Madt>> = SpinLock::new(None);

    /// `len` bytes of physical memory at `phys`, read through the HHDM.
    /// Refuses ranges outside the HHDM and memory that Rust code may own
    /// (usable or kernel/boot regions), where a shared reference could
    /// alias a mutable one.
    fn phys_bytes(info: &BootInfo, phys: u64, len: usize) -> Result<&'static [u8], AcpiError> {
        let end = phys.checked_add(len as u64).ok_or(AcpiError::BadLength)?;
        if phys == 0 || end > info.hhdm_size {
            return Err(AcpiError::BadLength);
        }
        if overlaps(
            info.regions(),
            phys,
            end,
            &[MemoryKind::Usable, MemoryKind::KernelAndBoot],
        ) {
            return Err(AcpiError::BadLength);
        }
        // SAFETY: the range is inside the HHDM, which maps it readable; it
        // is firmware memory (not usable RAM or the kernel's), which nothing
        // in STELA writes, so the shared reference cannot alias a mutable
        // one. ACPI reclaimable memory is never handed to the PMM.
        Ok(unsafe { core::slice::from_raw_parts((HHDM_BASE + phys) as *const u8, len) })
    }

    /// The checked table with `signature` at `phys`.
    fn table(info: &BootInfo, phys: u64, signature: &[u8; 4]) -> Result<&'static [u8], AcpiError> {
        let (_, length) = tables::header_info(phys_bytes(info, phys, tables::HEADER_LEN)?)?;
        if !(tables::HEADER_LEN..=MAX_TABLE).contains(&length) {
            return Err(AcpiError::BadLength);
        }
        tables::check_table(phys_bytes(info, phys, length)?, signature)
    }

    /// Finds and parses the MADT.
    fn find_madt(info: &BootInfo) -> Result<Madt, &'static str> {
        if info.rsdp_phys == 0 {
            return Err("no RSDP from the loader");
        }
        let rsdp = phys_bytes(info, info.rsdp_phys, tables::RSDP_V2_LEN)
            .and_then(tables::parse_rsdp)
            .map_err(|_| "bad RSDP")?;
        let (root_phys, wide) = rsdp.root();
        let root = table(info, root_phys, if wide { b"XSDT" } else { b"RSDT" })
            .map_err(|_| "bad XSDT/RSDT")?;
        for entry in tables::root_entries(root, wide) {
            let Ok(header) = phys_bytes(info, entry, tables::HEADER_LEN) else {
                continue;
            };
            if header.starts_with(b"APIC") {
                let madt = table(info, entry, b"APIC").map_err(|_| "bad MADT")?;
                return tables::parse_madt(madt).map_err(|_| "bad MADT entry");
            }
        }
        Err("no MADT")
    }

    /// Finds the MADT. STELA needs the APIC, so a missing or broken MADT is
    /// fatal. Called by `frame::init`.
    pub fn init(info: &'static BootInfo) {
        let madt = match find_madt(info) {
            Ok(madt) => madt,
            Err(why) => panic!("acpi: {why}; STELA needs ACPI and an APIC"),
        };
        crate::kprintln!(
            "acpi: local APIC {:#x}, {} processor(s), {} I/O APIC(s), {} override(s)",
            madt.local_apic_address,
            madt.processors.iter().filter(|p| p.enabled).count(),
            madt.io_apics.len(),
            madt.overrides.len()
        );
        *MADT.lock() = Some(Box::leak(Box::new(madt)));
    }

    /// The parsed MADT. Panics before `frame::init`.
    pub fn madt() -> &'static Madt {
        match *MADT.lock() {
            Some(madt) => madt,
            None => panic!("acpi: madt() before frame::init()"),
        }
    }
}
