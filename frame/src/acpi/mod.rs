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
//! NVS memory, which it covers). The MADT and the MCFG (PCI ECAM windows)
//! are parsed; the logic is in [`tables`] and tested on the host.

pub mod tables;

#[cfg(target_os = "none")]
pub use target::{init, madt, mcfg};

#[cfg(target_os = "none")]
mod target {
    use super::tables::{self, AcpiError, Madt, McfgEntry};
    use crate::mm::frames::overlaps;
    use crate::sync::SpinLock;
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use bootinfo::{BootInfo, HHDM_BASE, MemoryKind};

    /// Largest table accepted; real MADTs and XSDTs are a few KiB.
    const MAX_TABLE: usize = 1 << 20;

    static MADT: SpinLock<Option<&'static Madt>> = SpinLock::new(None);
    static MCFG: SpinLock<Option<&'static [McfgEntry]>> = SpinLock::new(None);

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

    /// The physical addresses of the tables the XSDT (or RSDT) lists.
    fn root_tables(info: &BootInfo) -> Result<Vec<u64>, &'static str> {
        if info.rsdp_phys == 0 {
            return Err("no RSDP from the loader");
        }
        let rsdp = phys_bytes(info, info.rsdp_phys, tables::RSDP_V2_LEN)
            .and_then(tables::parse_rsdp)
            .map_err(|_| "bad RSDP")?;
        let (root_phys, wide) = rsdp.root();
        let root = table(info, root_phys, if wide { b"XSDT" } else { b"RSDT" })
            .map_err(|_| "bad XSDT/RSDT")?;
        Ok(tables::root_entries(root, wide).collect())
    }

    /// The first listed table with `signature`, checked.
    fn find(info: &BootInfo, roots: &[u64], signature: &[u8; 4]) -> Option<&'static [u8]> {
        roots.iter().find_map(|&entry| {
            let header = phys_bytes(info, entry, tables::HEADER_LEN).ok()?;
            if header.starts_with(signature) {
                table(info, entry, signature).ok()
            } else {
                None
            }
        })
    }

    /// Finds and parses the MADT.
    fn find_madt(info: &BootInfo, roots: &[u64]) -> Result<Madt, &'static str> {
        let madt = find(info, roots, b"APIC").ok_or("no (valid) MADT")?;
        tables::parse_madt(madt).map_err(|_| "bad MADT entry")
    }

    /// Finds the MADT. STELA needs the APIC, so a missing or broken MADT is
    /// fatal. Called by `frame::init`.
    pub fn init(info: &'static BootInfo) {
        let madt = match root_tables(info).and_then(|roots| {
            let madt = find_madt(info, &roots)?;
            if let Some(mcfg) = find(info, &roots, b"MCFG") {
                match tables::parse_mcfg(mcfg) {
                    Ok(entries) => *MCFG.lock() = Some(Box::leak(entries.into_boxed_slice())),
                    Err(e) => crate::kprintln!("acpi: ignoring a bad MCFG ({e:?})"),
                }
            }
            Ok(madt)
        }) {
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

    /// The MCFG's ECAM windows, if the firmware has an MCFG (empty before
    /// `frame::init`).
    pub fn mcfg() -> &'static [McfgEntry] {
        (*MCFG.lock()).unwrap_or(&[])
    }

    /// The parsed MADT. Panics before `frame::init`.
    pub fn madt() -> &'static Madt {
        match *MADT.lock() {
            Some(madt) => madt,
            None => panic!("acpi: madt() before frame::init()"),
        }
    }
}
