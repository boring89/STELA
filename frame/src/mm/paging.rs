// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The kernel's page tables, edited through the HHDM.
//!
//! The loader built the tables (see `docs/boot.md`); the kernel keeps them,
//! removes the loader's identity map of the lower half and maps 4 KiB pages
//! in the dynamic region (`DYNAMIC_BASE`). Mapping and unmapping are limited
//! to that region, so safe code cannot disturb the kernel image or the HHDM.
//! Its upper half is the MMIO window: only [`super::mmio`] maps there, and
//! [`unmap`] refuses it (device pages are not frames).
//!
//! Entry format: Intel SDM Vol. 3A 4.5 "4-Level Paging and 5-Level Paging",
//! Tables 4-15 to 4-20; TLB maintenance: Vol. 3A 4.10.4 "Invalidation of TLBs
//! and Paging-Structure Caches".

use super::pmm::{self, PhysFrame};
use super::{DYNAMIC_BASE, MMIO_BASE, MMIO_SIZE};
use crate::sync::SpinLock;
use bootinfo::{BootInfo, HHDM_BASE};
use core::arch::asm;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
/// PWT: page-level write-through.
const WRITE_THROUGH: u64 = 1 << 3;
/// PCD: page-level cache disable.
const CACHE_DISABLE: u64 = 1 << 4;
/// PS: in a PDPTE or PDE, the entry maps a 1 GiB or 2 MiB page.
const HUGE: u64 = 1 << 7;
const NO_EXECUTE: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Whether the loader enabled `IA32_EFER.NXE`; without it bit 63 is
/// reserved and must stay 0.
static NX: AtomicBool = AtomicBool::new(false);

/// Serialises page-table edits.
static LOCK: SpinLock<()> = SpinLock::new(());

/// Permissions of a mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags {
    /// Writes allowed.
    pub writable: bool,
    /// Instruction fetches allowed.
    pub executable: bool,
}

impl Flags {
    /// Read/write data, not executable.
    pub const DATA: Flags = Flags {
        writable: true,
        executable: false,
    };
}

/// Why a mapping operation failed.
#[derive(Debug)]
pub enum MapError {
    /// The address is not 4 KiB aligned.
    Unaligned,
    /// The address is outside the allowed part of the dynamic region.
    OutOfRegion,
    /// Something is already mapped there (map).
    AlreadyMapped,
    /// Nothing is mapped there (unmap).
    NotMapped,
    /// No memory for a page table.
    OutOfMemory,
}

/// The 512 entries of the table at physical `phys`.
///
/// # Safety
///
/// `phys` must be a page table of the active hierarchy, the caller must
/// hold [`LOCK`] (or run before anyone else can), and must not let two of
/// these references to the same table be alive at once.
unsafe fn table(phys: u64) -> &'static mut [u64; 512] {
    // SAFETY: page tables are RAM, which the HHDM maps writable; exclusivity
    // is the caller's obligation.
    unsafe { &mut *((HHDM_BASE + phys) as *mut [u64; 512]) }
}

/// Physical address of the PML4 (CR3 bits 12..51, Vol. 3A 4.5.2).
fn root() -> u64 {
    let cr3: u64;
    // SAFETY: reading CR3 has no side effects.
    unsafe { asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags)) };
    cr3 & ADDR_MASK
}

/// Flushes the TLB entry for `virt`.
fn invlpg(virt: u64) {
    // SAFETY: invalidating a TLB entry only makes the processor re-read the
    // page tables.
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

/// Index into the table at `level` (4 = PML4 ... 1 = PT) for `virt`.
const fn index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * (level - 1))) & 0x1FF) as usize
}

/// Takes over the loader's tables: records NX support and removes the
/// identity map of the lower half (PML4 slots 0-255), which nothing uses
/// once the kernel runs.
pub(crate) fn init(info: &'static BootInfo) {
    NX.store(info.nx_enabled != 0, Ordering::Relaxed);
    let _guard = LOCK.lock();
    // SAFETY: the PML4 is the active root; LOCK is held.
    let pml4 = unsafe { table(root()) };
    pml4[..256].fill(0);
    // SAFETY: rewriting CR3 with its current value flushes all non-global
    // TLB entries (Vol. 3A 4.10.4.1), making the removal take effect. The
    // code, stack and data in use are all in the higher half.
    unsafe {
        asm!(
            "mov {tmp}, cr3",
            "mov cr3, {tmp}",
            tmp = out(reg) _,
            options(nostack, preserves_flags)
        )
    };
}

/// Checks that `virt` is a page inside `region`.
fn check(virt: u64, region: Range<u64>) -> Result<(), MapError> {
    if !virt.is_multiple_of(4096) {
        return Err(MapError::Unaligned);
    }
    if !region.contains(&virt) {
        return Err(MapError::OutOfRegion);
    }
    Ok(())
}

/// Maps the page at `virt` (in the dynamic region, below the MMIO window)
/// to `frame`, which the
/// mapping then owns until [`unmap`] returns it.
pub fn map(virt: u64, frame: PhysFrame, flags: Flags) -> Result<(), MapError> {
    check(virt, DYNAMIC_BASE..MMIO_BASE)?;
    let mut bits = PRESENT;
    if flags.writable {
        bits |= WRITABLE;
    }
    if !flags.executable && NX.load(Ordering::Relaxed) {
        bits |= NO_EXECUTE;
    }
    // On success the mapping owns the frame (the token ends here); `unmap`
    // hands out a new token.
    map_page(virt, frame.addr(), bits)
}

/// Maps the page at `virt` (in the dynamic region) to device memory at
/// `phys`: writable, not executable, uncached (PCD and PWT set: with the
/// power-on PAT that selects PAT entry 3, UC; Vol. 3A "Page Attribute
/// Table (PAT)", table "Selection of PAT Entries with PAT, PCD, and PWT
/// Flags"). `virt` must be in the MMIO window. Used by [`super::mmio`].
pub(crate) fn map_device(virt: u64, phys: u64) -> Result<(), MapError> {
    check(virt, MMIO_BASE..MMIO_BASE + MMIO_SIZE)?;
    let mut bits = PRESENT | WRITABLE | WRITE_THROUGH | CACHE_DISABLE;
    if NX.load(Ordering::Relaxed) {
        bits |= NO_EXECUTE;
    }
    map_page(virt, phys & ADDR_MASK, bits)
}

/// Installs the leaf entry `phys | bits` for `virt`, creating the missing
/// intermediate tables.
fn map_page(virt: u64, phys: u64, bits: u64) -> Result<(), MapError> {
    let _guard = LOCK.lock();
    let mut table_phys = root();
    for level in [4, 3, 2] {
        // SAFETY: `table_phys` is a table of the active hierarchy (the root,
        // or taken from a present non-huge entry); LOCK is held; the
        // reference ends before the next table is opened.
        let entry = &mut unsafe { table(table_phys) }[index(virt, level)];
        if *entry & PRESENT == 0 {
            let new = pmm::alloc_zeroed_frame().ok_or(MapError::OutOfMemory)?;
            // Intermediate entries allow everything; the leaf decides.
            // The table now belongs to the hierarchy; the token is consumed.
            *entry = new.addr() | PRESENT | WRITABLE;
        } else if *entry & HUGE != 0 {
            return Err(MapError::AlreadyMapped);
        }
        table_phys = *entry & ADDR_MASK;
    }
    // SAFETY: as above, for the page table (level 1).
    let entry = &mut unsafe { table(table_phys) }[index(virt, 1)];
    if *entry & PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    *entry = phys | bits;
    invlpg(virt);
    Ok(())
}

/// Removes the mapping of the page at `virt` (in the dynamic region, below
/// the MMIO window) and
/// returns its frame. Page tables that become empty are kept.
pub fn unmap(virt: u64) -> Result<PhysFrame, MapError> {
    // Not the MMIO window: its pages are device memory, not frames.
    check(virt, DYNAMIC_BASE..MMIO_BASE)?;
    let _guard = LOCK.lock();
    let mut phys = root();
    for level in [4, 3, 2] {
        // SAFETY: see `map`.
        let entry = unsafe { table(phys) }[index(virt, level)];
        if entry & PRESENT == 0 || entry & HUGE != 0 {
            return Err(MapError::NotMapped);
        }
        phys = entry & ADDR_MASK;
    }
    // SAFETY: see `map`.
    let entry = &mut unsafe { table(phys) }[index(virt, 1)];
    if *entry & PRESENT == 0 {
        return Err(MapError::NotMapped);
    }
    let frame = PhysFrame::from_addr(*entry & ADDR_MASK);
    *entry = 0;
    invlpg(virt);
    Ok(frame)
}

/// The physical address `virt` maps to, if any (any address; 4 KiB,
/// 2 MiB and 1 GiB pages).
pub fn translate(virt: u64) -> Option<u64> {
    let _guard = LOCK.lock();
    let mut phys = root();
    for level in [4, 3, 2, 1] {
        // SAFETY: see `map`; this only reads.
        let entry = unsafe { table(phys) }[index(virt, level)];
        if entry & PRESENT == 0 {
            return None;
        }
        // Bits below the page size come from the address.
        let page_bits = 12 + 9 * (level - 1);
        if level == 1 || entry & HUGE != 0 {
            let base = entry & ADDR_MASK & !((1u64 << page_bits) - 1);
            return Some(base | (virt & ((1u64 << page_bits) - 1)));
        }
        phys = entry & ADDR_MASK;
    }
    None
}
