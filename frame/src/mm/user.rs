// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! User address spaces: one PML4 per process. The upper half (slots
//! 256-511) is copied from the kernel's PML4, so every address space maps
//! the kernel the same way and shares its lower-level tables; the lower
//! half (`0..USER_END`) belongs to the process.
//!
//! ```text
//!  process PML4                       kernel PML4
//!  ┌─────────────┐                    ┌─────────────┐
//!  │ 0..255 own  │──► own PDPT/PD/PT  │ 0..255 empty│
//!  ├─────────────┤                    ├─────────────┤
//!  │ 256..511    │──► the same tables as the kernel's slots 256..511
//!  └─────────────┘                    └─────────────┘
//! ```
//!
//! User pages have U/S = 1 at every level (Intel SDM Vol. 3A 4.6 "Access
//! Rights"); kernel pages have U/S = 0, so ring 3 cannot touch them. Every
//! frame mapped into an address space is owned by it and returned to the
//! PMM, with the page tables, when it is dropped.
//!
//! The kernel reads and writes user memory through the HHDM after walking
//! the tables ([`AddressSpace::read`], [`AddressSpace::write`]), never
//! through the user address itself, so a bad pointer from a system call
//! is an error, not a page fault in the kernel.

use super::USER_END;
use super::paging::{
    self, ADDR_MASK, HUGE, MapError, NO_EXECUTE, PRESENT, USER, WRITABLE, index, table,
};
use super::pmm::{self, PhysFrame};
use crate::sync::SpinLock;
use bootinfo::HHDM_BASE;

/// Permissions of a user page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserFlags {
    /// Writes allowed.
    pub writable: bool,
    /// Instruction fetches allowed (honoured only when NX is available).
    pub executable: bool,
}

/// A user pointer that is not mapped, not user-accessible, not writable
/// when it should be, or outside the user half.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadAddress;

/// A process's page tables.
pub struct AddressSpace {
    /// Physical address of the PML4 (owned).
    root: u64,
    /// Serialises edits and walks of the user half.
    lock: SpinLock<()>,
}

impl core::fmt::Debug for AddressSpace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "AddressSpace({:#x})", self.root)
    }
}

/// The page containing `addr`.
const fn page_of(addr: u64) -> u64 {
    addr & !0xFFF
}

impl AddressSpace {
    /// An empty user half and the kernel's upper half.
    pub fn new() -> Result<AddressSpace, MapError> {
        let frame = pmm::alloc_zeroed_frame().ok_or(MapError::OutOfMemory)?;
        let root = frame.addr();
        // The token ends here: the frame is the new PML4, freed in Drop.
        let _guard = paging::kernel_lock();
        // SAFETY: `root` was just allocated and zeroed, so nothing else
        // refers to it; the kernel PML4 is only read, under its lock.
        unsafe {
            let kernel = *table(paging::kernel_root());
            table(root)[256..].copy_from_slice(&kernel[256..]);
        }
        Ok(AddressSpace {
            root,
            lock: SpinLock::new(()),
        })
    }

    /// Physical address of the PML4.
    pub fn root(&self) -> u64 {
        self.root
    }

    /// Maps the user page at `virt` to `frame`, which the address space
    /// then owns.
    pub fn map(&self, virt: u64, frame: PhysFrame, flags: UserFlags) -> Result<(), MapError> {
        if !virt.is_multiple_of(4096) {
            return Err(MapError::Unaligned);
        }
        if virt >= USER_END {
            return Err(MapError::OutOfRegion);
        }
        let mut leaf = frame.addr() | PRESENT | USER;
        if flags.writable {
            leaf |= WRITABLE;
        }
        if !flags.executable && paging::nx() {
            leaf |= NO_EXECUTE;
        }
        let _guard = self.lock.lock();
        let mut table_phys = self.root;
        for level in [4, 3, 2] {
            // SAFETY: tables of this address space's user half (slot < 256),
            // edited under its lock; one reference at a time.
            let entry = &mut unsafe { table(table_phys) }[index(virt, level)];
            if *entry & PRESENT == 0 {
                let new = pmm::alloc_zeroed_frame().ok_or(MapError::OutOfMemory)?;
                // Owned by the hierarchy from now on; freed in Drop.
                *entry = new.addr() | PRESENT | WRITABLE | USER;
            }
            table_phys = *entry & ADDR_MASK;
        }
        // SAFETY: as above, for the page table.
        let entry = &mut unsafe { table(table_phys) }[index(virt, 1)];
        if *entry & PRESENT != 0 {
            return Err(MapError::AlreadyMapped);
        }
        *entry = leaf;
        if self.is_active() {
            paging::invlpg(virt);
        }
        Ok(())
    }

    /// Removes the mapping of the user page at `virt` and returns its frame.
    pub fn unmap(&self, virt: u64) -> Result<PhysFrame, MapError> {
        if !virt.is_multiple_of(4096) {
            return Err(MapError::Unaligned);
        }
        if virt >= USER_END {
            return Err(MapError::OutOfRegion);
        }
        let _guard = self.lock.lock();
        let mut phys = self.root;
        for level in [4, 3, 2] {
            // SAFETY: see `map`.
            let entry = unsafe { table(phys) }[index(virt, level)];
            if entry & PRESENT == 0 {
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
        if self.is_active() {
            paging::invlpg(virt);
        }
        Ok(frame)
    }

    /// The leaf entry for the user page containing `virt`, if mapped.
    fn leaf(&self, virt: u64) -> Option<u64> {
        if virt >= USER_END {
            return None;
        }
        let _guard = self.lock.lock();
        let mut phys = self.root;
        for level in [4, 3, 2, 1] {
            // SAFETY: see `map`; this only reads.
            let entry = unsafe { table(phys) }[index(virt, level)];
            if entry & PRESENT == 0 || entry & USER == 0 {
                return None;
            }
            if level == 1 {
                return Some(entry);
            }
            if entry & HUGE != 0 {
                // User halves only hold 4 KiB pages.
                return None;
            }
            phys = entry & ADDR_MASK;
        }
        None
    }

    /// The physical address `virt` maps to, if it is a mapped user page.
    pub fn translate(&self, virt: u64) -> Option<u64> {
        self.leaf(virt)
            .map(|entry| (entry & ADDR_MASK) | (virt & 0xFFF))
    }

    /// Calls `f(physical address, offset in buf, length)` for each piece of
    /// `virt..virt + len` that falls in one page, after checking that every
    /// page is a mapped user page (writable too, with `write`). Nothing is
    /// called if any check fails.
    fn pieces(
        &self,
        virt: u64,
        len: usize,
        write: bool,
        mut f: impl FnMut(u64, usize, usize),
    ) -> Result<(), BadAddress> {
        if len == 0 {
            return Ok(());
        }
        let end = virt.checked_add(len as u64).ok_or(BadAddress)?;
        if end > USER_END {
            return Err(BadAddress);
        }
        let mut page = page_of(virt);
        while page < end {
            let entry = self.leaf(page).ok_or(BadAddress)?;
            if write && entry & WRITABLE == 0 {
                return Err(BadAddress);
            }
            page += 4096;
        }
        let mut at = virt;
        while at < end {
            let piece = (page_of(at) + 4096).min(end) - at;
            let phys = self.translate(at).ok_or(BadAddress)?;
            f(phys, (at - virt) as usize, piece as usize);
            at += piece;
        }
        Ok(())
    }

    /// Whether `virt..virt + len` is mapped user memory (and writable, with
    /// `write`).
    pub fn check(&self, virt: u64, len: usize, write: bool) -> bool {
        self.pieces(virt, len, write, |_, _, _| {}).is_ok()
    }

    /// Copies user memory at `virt` into `buf` (copy_from_user).
    pub fn read(&self, virt: u64, buf: &mut [u8]) -> Result<(), BadAddress> {
        self.pieces(virt, buf.len(), false, |phys, offset, len| {
            // SAFETY: `phys..phys + len` is inside one frame owned by this
            // address space (checked by `pieces`); the HHDM maps it. User
            // threads may write it concurrently on another processor; the
            // bytes are copied as plain data.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (HHDM_BASE + phys) as *const u8,
                    buf[offset..].as_mut_ptr(),
                    len,
                )
            };
        })
    }

    /// Copies `data` into user memory at `virt` (copy_to_user). The pages
    /// must be writable.
    pub fn write(&self, virt: u64, data: &[u8]) -> Result<(), BadAddress> {
        self.write_inner(virt, data, true)
    }

    /// Copies `data` into user pages at `virt` whatever their write
    /// permission (to load a program into read-only pages).
    pub fn load(&self, virt: u64, data: &[u8]) -> Result<(), BadAddress> {
        self.write_inner(virt, data, false)
    }

    fn write_inner(&self, virt: u64, data: &[u8], need_write: bool) -> Result<(), BadAddress> {
        self.pieces(virt, data.len(), need_write, |phys, offset, len| {
            // SAFETY: as in `read`; the frame is RAM owned by this address
            // space and no Rust reference points into it.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data[offset..].as_ptr(),
                    (HHDM_BASE + phys) as *mut u8,
                    len,
                )
            };
        })
    }

    /// Makes this the processor's address space (writes CR3 if it is not
    /// already).
    pub fn activate(&self) {
        if !self.is_active() {
            // SAFETY: the PML4's upper half is the kernel's (copied in
            // `new`), and `Drop` switches away before freeing it.
            unsafe { paging::load_root(self.root) };
        }
    }

    /// Whether CR3 points at this address space.
    pub fn is_active(&self) -> bool {
        paging::root() == self.root
    }
}

/// Frees the user half of the table at `phys` (level 4..1) and below.
///
/// # Safety
///
/// The table must belong to a user half that nothing uses any more.
unsafe fn free_tables(phys: u64, level: u32) {
    let entries = if level == 4 { 0..256 } else { 0..512 };
    for i in entries {
        // SAFETY: the caller's guarantee; one reference at a time.
        let entry = unsafe { table(phys) }[i];
        if entry & PRESENT == 0 {
            continue;
        }
        let next = entry & ADDR_MASK;
        if level > 1 {
            // SAFETY: a lower table of the same unused hierarchy.
            unsafe { free_tables(next, level - 1) };
        } else {
            pmm::free_frame(PhysFrame::from_addr(next));
            continue;
        }
        // The lower table itself.
        pmm::free_frame(PhysFrame::from_addr(next));
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        let _irq = crate::sync::IrqGuard::new();
        if self.is_active() {
            // SAFETY: the kernel's PML4 maps everything the kernel uses.
            unsafe { paging::load_root(paging::kernel_root()) };
        }
        // SAFETY: not active (any more), and `&mut self` means no other
        // user of the tables; the upper half (shared) is not touched.
        unsafe { free_tables(self.root, 4) };
        pmm::free_frame(PhysFrame::from_addr(self.root));
    }
}
