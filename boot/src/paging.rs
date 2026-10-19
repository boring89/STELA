// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Builds the kernel's initial 4-level page tables while boot services are
//! still running.
//!
//! UEFI identity-maps memory (virtual = physical) on x86-64 (UEFI
//! Specification 2.10, 2.3.4 "x64 Platforms"), so a table's address in the
//! loader is also its physical address, which is what the entries store.
//! Tables are allocated as whole pages of type `LOADER_DATA` through the
//! global allocator and never freed: the kernel inherits them.
//!
//! Entry format: Intel SDM Vol. 3A 4.5 "4-Level Paging and 5-Level Paging",
//! Tables 4-15 (PML4E), 4-17 (PDPTE), 4-18 (PDE mapping a 2 MiB page),
//! 4-19 (PDE referencing a page table), 4-20 (PTE).

use alloc::boxed::Box;
use alloc::vec::Vec;

/// Bit 0: the entry is present.
pub const PRESENT: u64 = 1 << 0;
/// Bit 1: writes are allowed.
pub const WRITABLE: u64 = 1 << 1;
/// Bit 7 in a PDE: the entry maps a 2 MiB page instead of a page table.
const HUGE: u64 = 1 << 7;
/// Bit 63: instruction fetches are not allowed (needs `IA32_EFER.NXE`).
pub const NO_EXECUTE: u64 = 1 << 63;
/// Bits 12..=51: physical address of the next table or of the page.
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// 4 KiB.
pub const PAGE_4K: u64 = 4096;
/// 2 MiB.
pub const PAGE_2M: u64 = 2 * 1024 * 1024;

/// One page table of any level: 512 eight-byte entries, page aligned.
#[repr(C, align(4096))]
struct Table([u64; 512]);

/// Index into the table at `level` (4 = PML4 ... 1 = PT) for `virt`:
/// 9 bits each, starting at bit 39, 30, 21, 12 (Intel SDM Vol. 3A Figure 4-8).
const fn index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * (level - 1))) & 0x1FF) as usize
}

/// Why a mapping could not be created.
#[derive(Debug)]
pub enum MapError {
    /// The address is not aligned to the page size.
    Unaligned,
    /// Part of the range is already mapped with a different page size.
    Conflict,
}

/// A set of page tables under one PML4.
pub struct PageTables {
    /// `tables[0]` is the PML4.
    tables: Vec<&'static mut Table>,
}

impl PageTables {
    /// Creates an empty PML4.
    pub fn new() -> PageTables {
        let mut tables = PageTables { tables: Vec::new() };
        tables.alloc();
        tables
    }

    /// Physical address of the PML4: the value for CR3.
    pub fn root(&self) -> u64 {
        address(self.tables[0])
    }

    /// Number of 4 KiB pages the tables occupy.
    pub fn page_count(&self) -> usize {
        self.tables.len()
    }

    fn alloc(&mut self) -> usize {
        self.tables.push(Box::leak(Box::new(Table([0; 512]))));
        self.tables.len() - 1
    }

    fn find(&self, phys: u64) -> Option<usize> {
        self.tables.iter().position(|t| address(t) == phys)
    }

    /// Returns the table that entry `slot` of table `parent` points to,
    /// creating it if the entry is empty. Intermediate entries allow
    /// everything (present, writable, executable); the leaf decides.
    fn child(&mut self, parent: usize, slot: usize) -> Result<usize, MapError> {
        let entry = self.tables[parent].0[slot];
        if entry & PRESENT != 0 {
            if entry & HUGE != 0 {
                return Err(MapError::Conflict);
            }
            return self.find(entry & ADDR_MASK).ok_or(MapError::Conflict);
        }
        let child = self.alloc();
        self.tables[parent].0[slot] = address(self.tables[child]) | PRESENT | WRITABLE;
        Ok(child)
    }

    /// Maps the 4 KiB page at `virt` to `phys` with `flags` (`WRITABLE`,
    /// `NO_EXECUTE`; `PRESENT` is added).
    pub fn map_4k(&mut self, virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
        if !virt.is_multiple_of(PAGE_4K) || !phys.is_multiple_of(PAGE_4K) {
            return Err(MapError::Unaligned);
        }
        let pdpt = self.child(0, index(virt, 4))?;
        let pd = self.child(pdpt, index(virt, 3))?;
        let pt = self.child(pd, index(virt, 2))?;
        let slot = &mut self.tables[pt].0[index(virt, 1)];
        if *slot & PRESENT != 0 {
            return Err(MapError::Conflict);
        }
        *slot = phys | flags | PRESENT;
        Ok(())
    }

    /// Maps the 2 MiB page at `virt` to `phys` with `flags`.
    pub fn map_2m(&mut self, virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
        if !virt.is_multiple_of(PAGE_2M) || !phys.is_multiple_of(PAGE_2M) {
            return Err(MapError::Unaligned);
        }
        let pdpt = self.child(0, index(virt, 4))?;
        let pd = self.child(pdpt, index(virt, 3))?;
        let slot = &mut self.tables[pd].0[index(virt, 2)];
        if *slot & PRESENT != 0 {
            return Err(MapError::Conflict);
        }
        *slot = phys | flags | HUGE | PRESENT;
        Ok(())
    }

    /// Maps `size` bytes of physical memory starting at 0 to `virt_base`
    /// with 2 MiB pages.
    pub fn map_linear_2m(&mut self, virt_base: u64, size: u64, flags: u64) -> Result<(), MapError> {
        let mut offset = 0;
        while offset < size {
            self.map_2m(virt_base + offset, offset, flags)?;
            offset += PAGE_2M;
        }
        Ok(())
    }
}

/// Address of a table; equal to its physical address under UEFI's identity
/// mapping.
fn address(table: &Table) -> u64 {
    table as *const Table as u64
}
