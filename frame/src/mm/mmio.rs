// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Memory-mapped I/O: device registers that live at physical addresses
//! (the local APIC at `0xFEE0_0000`, the I/O APIC at `0xFEC0_0000`, ...).
//!
//! The HHDM maps RAM write-back cached, which is wrong for registers: a
//! cached read could return a stale value and writes could be combined or
//! delayed. [`map`] gives a device range its own uncached mapping in the
//! MMIO window of the dynamic region (see `mm/mod.rs`), handing out virtual
//! addresses with a bump pointer that never goes back (mappings are never
//! removed; devices are mapped once at boot).
//!
//! [`MmioRegion`] is a token separate from `PhysFrame`: device memory is not
//! RAM and never goes back to the frame allocator. Its accessors use
//! volatile reads and writes so that the compiler neither drops nor merges
//! them, and they check the offset, so safe code can only reach the range
//! it mapped. Ranges that overlap RAM in the memory map are refused.

use super::frames::overlaps_ram;
use super::paging::{self, MapError};
use super::{MMIO_BASE, MMIO_SIZE};
use crate::sync::SpinLock;
use bootinfo::{BootInfo, PAGE_SIZE};

/// The next free virtual address of the MMIO window.
static NEXT: SpinLock<u64> = SpinLock::new(MMIO_BASE);
/// The memory map, to refuse ranges that are RAM.
static BOOT_INFO: SpinLock<Option<&'static BootInfo>> = SpinLock::new(None);

/// Why [`map`] failed.
#[derive(Debug)]
pub enum MmioError {
    /// `len` is 0, or `phys + len` overflows.
    BadRange,
    /// The range overlaps RAM in the memory map.
    Ram,
    /// The MMIO window is used up.
    NoSpace,
    /// The page tables could not be updated.
    Map(MapError),
    /// Called before `frame::init`.
    NotReady,
}

/// Remembers the memory map. Called by `mm::init`.
pub(crate) fn init(info: &'static BootInfo) {
    *BOOT_INFO.lock() = Some(info);
}

/// Maps the physical range `phys..phys + len` uncached (and not executable)
/// and returns access to it. `phys` need not be page aligned.
pub fn map(phys: u64, len: usize) -> Result<MmioRegion, MmioError> {
    let end = phys
        .checked_add(len as u64)
        .filter(|_| len > 0)
        .ok_or(MmioError::BadRange)?;
    let info = (*BOOT_INFO.lock()).ok_or(MmioError::NotReady)?;
    if overlaps_ram(info.regions(), phys, end) {
        return Err(MmioError::Ram);
    }
    let first_page = phys & !(PAGE_SIZE - 1);
    let bytes = end.div_ceil(PAGE_SIZE) * PAGE_SIZE - first_page;
    let virt_start = {
        let mut next = NEXT.lock();
        let start = *next;
        if bytes > MMIO_BASE + MMIO_SIZE - start {
            return Err(MmioError::NoSpace);
        }
        *next = start + bytes;
        start
    };
    for page in (0..bytes).step_by(PAGE_SIZE as usize) {
        paging::map_device(virt_start + page, first_page + page).map_err(MmioError::Map)?;
    }
    Ok(MmioRegion {
        virt: virt_start + (phys - first_page),
        phys,
        len,
    })
}

/// A mapped device range (see [`map`]). Offsets are relative to the
/// physical address given to [`map`].
#[derive(Debug)]
pub struct MmioRegion {
    virt: u64,
    phys: u64,
    len: usize,
}

impl MmioRegion {
    /// The physical address of the start of the range.
    pub fn phys(&self) -> u64 {
        self.phys
    }

    /// The virtual address of the start of the range.
    pub fn virt(&self) -> u64 {
        self.virt
    }

    /// Length of the range in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Always false: [`map`] refuses empty ranges.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The address of the 32-bit register at `offset`. Panics unless it is
    /// 4-byte aligned and inside the range.
    fn register(&self, offset: usize) -> *mut u32 {
        assert!(
            offset.is_multiple_of(4) && offset.checked_add(4).is_some_and(|e| e <= self.len),
            "mmio: offset {offset:#x} outside the {:#x}-byte range at {:#x}",
            self.len,
            self.phys
        );
        (self.virt + offset as u64) as *mut u32
    }

    /// Reads the 32-bit register at `offset` (panics if out of range or
    /// unaligned).
    pub fn read_u32(&self, offset: usize) -> u32 {
        let register = self.register(offset);
        // SAFETY: `register` is aligned and inside the range `map` mapped
        // (checked above); the mapping is never removed and is device
        // memory, not RAM that Rust code owns.
        unsafe { register.read_volatile() }
    }

    /// Writes the 32-bit register at `offset` (panics if out of range or
    /// unaligned).
    pub fn write_u32(&self, offset: usize, value: u32) {
        let register = self.register(offset);
        // SAFETY: as in `read_u32`; the mapping is writable.
        unsafe { register.write_volatile(value) }
    }

    /// The address of the `size`-byte register at `offset`. Panics unless
    /// it is `size`-aligned and inside the range.
    fn small_register(&self, offset: usize, size: usize) -> u64 {
        assert!(
            offset.is_multiple_of(size) && offset.checked_add(size).is_some_and(|e| e <= self.len),
            "mmio: offset {offset:#x} outside the {:#x}-byte range at {:#x}",
            self.len,
            self.phys
        );
        self.virt + offset as u64
    }

    /// Reads the 16-bit register at `offset` (panics if out of range or
    /// unaligned). Some devices want each field read at its own width
    /// (virtio, VIRTIO 1.1 4.1.3.1).
    pub fn read_u16(&self, offset: usize) -> u16 {
        let register = self.small_register(offset, 2) as *const u16;
        // SAFETY: as in `read_u32`: aligned, inside the mapped device
        // range, which is never removed.
        unsafe { register.read_volatile() }
    }

    /// Writes the 16-bit register at `offset` (panics if out of range or
    /// unaligned).
    pub fn write_u16(&self, offset: usize, value: u16) {
        let register = self.small_register(offset, 2) as *mut u16;
        // SAFETY: as in `read_u16`; the mapping is writable.
        unsafe { register.write_volatile(value) }
    }

    /// Reads the 8-bit register at `offset` (panics if out of range).
    pub fn read_u8(&self, offset: usize) -> u8 {
        let register = self.small_register(offset, 1) as *const u8;
        // SAFETY: as in `read_u16`.
        unsafe { register.read_volatile() }
    }

    /// Writes the 8-bit register at `offset` (panics if out of range).
    pub fn write_u8(&self, offset: usize, value: u8) {
        assert!(
            offset < self.len,
            "mmio: offset {offset:#x} outside the {:#x}-byte range at {:#x}",
            self.len,
            self.phys
        );
        let register = (self.virt + offset as u64) as *mut u8;
        // SAFETY: inside the range `map` mapped (checked above), which is
        // device memory, never removed, and writable.
        unsafe { register.write_volatile(value) }
    }
}
