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
//! Rights"); kernel pages have U/S = 0, so ring 3 cannot touch them. A
//! frame mapped with [`AddressSpace::map`] is owned by the address space
//! and returned to the PMM, with the page tables, when it is dropped.
//! Frames of a [`SharedMemory`] mapped with [`AddressSpace::map_shared`]
//! are only borrowed: their entries carry the software bit [`SHARED`]
//! (bit 9, ignored by the processor: Vol. 3A Table 4-19), so the teardown
//! skips them, and the address space keeps a clone of the object until the
//! mapping is removed.
//!
//! The kernel reads and writes user memory through the HHDM after walking
//! the tables ([`AddressSpace::read`], [`AddressSpace::write`]), never
//! through the user address itself, so a bad pointer from a system call
//! is an error, not a page fault in the kernel.

use super::USER_END;
use super::device::DeviceRange;
use super::paging::{
    self, ADDR_MASK, CACHE_DISABLE, HUGE, MapError, NO_EXECUTE, PRESENT, USER, WRITABLE,
    WRITE_THROUGH, index, table,
};
use super::pmm::{self, PhysFrame};
use super::shared::SharedMemory;
use crate::sync::SpinLock;
use alloc::vec::Vec;
use bootinfo::HHDM_BASE;

/// Software bit of a leaf entry: the frame belongs to a [`SharedMemory`],
/// not to the address space.
const SHARED: u64 = 1 << 9;
/// Software bit (P13 K6): the frame is shared copy-on-write since a `fork`
/// (`super::cow`); the entry is read-only until a write copies it.
const COW: u64 = 1 << 10;
/// Software bit (P13 K6): a [`COW`] page that is writable once copied.
const WAS_WRITABLE: u64 = 1 << 11;

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
    /// Shared-memory mappings: start address and the object.
    shared: SpinLock<Vec<(u64, SharedMemory)>>,
    /// Device-memory mappings: start address and the range.
    devices: SpinLock<Vec<(u64, DeviceRange)>>,
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
            shared: SpinLock::new(Vec::new()),
            devices: SpinLock::new(Vec::new()),
        })
    }

    /// Physical address of the PML4.
    pub fn root(&self) -> u64 {
        self.root
    }

    /// The leaf entry bits for `phys` with `flags`.
    fn leaf_bits(phys: u64, flags: UserFlags) -> u64 {
        let mut leaf = phys | PRESENT | USER;
        if flags.writable {
            leaf |= WRITABLE;
        }
        if !flags.executable && paging::nx() {
            leaf |= NO_EXECUTE;
        }
        leaf
    }

    /// Writes the leaf entry `leaf` for the user page `virt`, creating the
    /// tables on the way. The caller holds `self.lock`.
    fn set_leaf(&self, virt: u64, leaf: u64) -> Result<(), MapError> {
        if !virt.is_multiple_of(4096) {
            return Err(MapError::Unaligned);
        }
        if virt >= USER_END {
            return Err(MapError::OutOfRegion);
        }
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

    /// Clears the leaf entry of the user page `virt` and returns it. Shared
    /// pages are only cleared with `shared`, own pages only without. The
    /// caller holds `self.lock`.
    fn clear_leaf(&self, virt: u64, shared: bool) -> Result<u64, MapError> {
        if !virt.is_multiple_of(4096) {
            return Err(MapError::Unaligned);
        }
        if virt >= USER_END {
            return Err(MapError::OutOfRegion);
        }
        let mut phys = self.root;
        for level in [4, 3, 2] {
            // SAFETY: see `set_leaf`.
            let entry = unsafe { table(phys) }[index(virt, level)];
            if entry & PRESENT == 0 {
                return Err(MapError::NotMapped);
            }
            phys = entry & ADDR_MASK;
        }
        // SAFETY: see `set_leaf`.
        let entry = &mut unsafe { table(phys) }[index(virt, 1)];
        if *entry & PRESENT == 0 || (*entry & SHARED != 0) != shared {
            return Err(MapError::NotMapped);
        }
        let old = *entry;
        *entry = 0;
        if self.is_active() {
            paging::invlpg(virt);
        }
        Ok(old)
    }

    /// Maps the user page at `virt` to `frame`, which the address space
    /// then owns.
    pub fn map(&self, virt: u64, frame: PhysFrame, flags: UserFlags) -> Result<(), MapError> {
        let leaf = Self::leaf_bits(frame.addr(), flags);
        let _guard = self.lock.lock();
        let result = self.set_leaf(virt, leaf);
        if result.is_err() {
            // Not mapped: the frame goes back.
            pmm::free_frame(frame);
        }
        result
    }

    /// Removes the mapping of the user page at `virt` and returns its frame.
    /// Pages of shared memory are refused (`NotMapped`); see
    /// [`AddressSpace::unmap_shared`].
    pub fn unmap(&self, virt: u64) -> Result<PhysFrame, MapError> {
        let _guard = self.lock.lock();
        let old = self.clear_leaf(virt, false)?;
        Ok(PhysFrame::from_addr(old & ADDR_MASK))
    }

    /// Changes the permissions of the user page at `virt` (one the address
    /// space owns; shared pages are refused with `NotMapped`). A page
    /// shared copy-on-write stays read-only; "writable" is remembered for
    /// when it is copied.
    pub fn protect(&self, virt: u64, flags: UserFlags) -> Result<(), MapError> {
        let _guard = self.lock.lock();
        let old = self.clear_leaf(virt, false)?;
        let leaf = if old & COW != 0 {
            let ro = UserFlags {
                writable: false,
                ..flags
            };
            let was = if flags.writable { WAS_WRITABLE } else { 0 };
            Self::leaf_bits(old & ADDR_MASK, ro) | COW | was
        } else {
            Self::leaf_bits(old & ADDR_MASK, flags)
        };
        // The tables exist (the page was mapped), so this cannot fail.
        self.set_leaf(virt, leaf)
    }

    /// Where the leaf entry of user page `virt` is: its page table and
    /// the index in it, if the tables exist. The caller holds `self.lock`.
    fn leaf_slot(&self, virt: u64) -> Option<(u64, usize)> {
        if virt >= USER_END {
            return None;
        }
        let mut phys = self.root;
        for level in [4, 3, 2] {
            // SAFETY: see `set_leaf`; this only reads.
            let entry = unsafe { table(phys) }[index(virt, level)];
            if entry & PRESENT == 0 {
                return None;
            }
            phys = entry & ADDR_MASK;
        }
        Some((phys, index(virt, 1)))
    }

    /// Gives page `virt` its own frame if it is shared copy-on-write
    /// (P13 K6): copies it when others still use the frame, or takes it
    /// over when this is the last reference. With `write`, only a page
    /// that was writable qualifies, and it becomes writable; without, it
    /// keeps its permission (the kernel loading data into it). False if
    /// the page is not such a page (or memory ran out).
    fn unshare(&self, virt: u64, write: bool) -> bool {
        let virt = page_of(virt);
        let _guard = self.lock.lock();
        let Some((table_phys, slot)) = self.leaf_slot(virt) else {
            return false;
        };
        // SAFETY: this address space's page table, edited under its lock;
        // the reference lives until the end of this function only.
        let entry = &mut unsafe { table(table_phys) }[slot];
        let old = *entry;
        if old & PRESENT == 0 || old & COW == 0 || (write && old & WAS_WRITABLE == 0) {
            return false;
        }
        let phys = old & ADDR_MASK;
        let mut leaf = old & !(ADDR_MASK | COW | WAS_WRITABLE);
        if old & WAS_WRITABLE != 0 {
            leaf |= WRITABLE;
        }
        let last = super::cow::COUNTS.lock().count(phys) == 1;
        if last {
            *entry = leaf | phys;
        } else {
            let Some(frame) = pmm::alloc_frame() else {
                return false;
            };
            // SAFETY: both frames are RAM reached through the HHDM; the
            // new one is ours alone, the old one is only read (every
            // other mapping of it is read-only).
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (HHDM_BASE + phys) as *const u8,
                    (HHDM_BASE + frame.addr()) as *mut u8,
                    4096,
                )
            };
            *entry = leaf | frame.addr();
            // One reference less to the old frame (never the last: count
            // was at least 2).
            pmm::free_frame(PhysFrame::from_addr(phys));
        }
        if self.is_active() {
            paging::invlpg(virt);
        }
        true
    }

    /// A write fault at `virt` in ring 3 (P13 K6): if the page is shared
    /// copy-on-write and was writable, it gets its own frame and the
    /// instruction can run again (true).
    pub fn resolve_cow(&self, virt: u64) -> bool {
        self.unshare(virt, true)
    }

    /// Maps all of `memory` at `virt..virt + memory.size()`. The address
    /// space keeps a clone of `memory` until [`AddressSpace::unmap_shared`]
    /// or its own end. Nothing stays mapped on failure.
    pub fn map_shared(
        &self,
        virt: u64,
        memory: &SharedMemory,
        flags: UserFlags,
    ) -> Result<(), MapError> {
        let end = virt
            .checked_add(memory.size())
            .ok_or(MapError::OutOfRegion)?;
        if end > USER_END {
            return Err(MapError::OutOfRegion);
        }
        let _guard = self.lock.lock();
        for i in 0..memory.pages() {
            let leaf = Self::leaf_bits(memory.frame_addr(i), flags) | SHARED;
            if let Err(e) = self.set_leaf(virt + i as u64 * 4096, leaf) {
                for j in 0..i {
                    let _ = self.clear_leaf(virt + j as u64 * 4096, true);
                }
                return Err(e);
            }
        }
        self.shared.lock().push((virt, memory.clone()));
        Ok(())
    }

    /// Maps the device memory `range` at `virt..virt + range.size()`, not
    /// executable, writable with `writable`, write-combining with
    /// `write_combining` when the PAT allows it (`arch::x86_64::pat`) and
    /// uncached otherwise. The pages are borrowed (marked [`SHARED`]), so
    /// they are never given to the PMM. Nothing stays mapped on failure.
    pub fn map_device(
        &self,
        virt: u64,
        range: &DeviceRange,
        writable: bool,
        write_combining: bool,
    ) -> Result<(), MapError> {
        let end = virt
            .checked_add(range.size())
            .ok_or(MapError::OutOfRegion)?;
        if end > USER_END {
            return Err(MapError::OutOfRegion);
        }
        let caching = if write_combining && crate::arch::x86_64::pat::wc_enabled() {
            crate::arch::x86_64::pat::PTE_PAT
        } else {
            CACHE_DISABLE | WRITE_THROUGH
        };
        let flags = UserFlags {
            writable,
            executable: false,
        };
        let _guard = self.lock.lock();
        for i in 0..range.pages() {
            let phys = range.phys() + i as u64 * 4096;
            let leaf = Self::leaf_bits(phys, flags) | SHARED | caching;
            if let Err(e) = self.set_leaf(virt + i as u64 * 4096, leaf) {
                for j in 0..i {
                    let _ = self.clear_leaf(virt + j as u64 * 4096, true);
                }
                return Err(e);
            }
        }
        self.devices.lock().push((virt, range.clone()));
        Ok(())
    }

    /// Removes the device mapping that starts at `virt` (made by
    /// [`AddressSpace::map_device`]) and returns its range.
    pub fn unmap_device(&self, virt: u64) -> Result<DeviceRange, MapError> {
        let range = {
            let mut devices = self.devices.lock();
            let at = devices
                .iter()
                .position(|(start, _)| *start == virt)
                .ok_or(MapError::NotMapped)?;
            devices.swap_remove(at).1
        };
        let _guard = self.lock.lock();
        for i in 0..range.pages() {
            let _ = self.clear_leaf(virt + i as u64 * 4096, true);
        }
        Ok(range)
    }

    /// Whether the user page at `virt` is mapped write-combining.
    pub fn is_write_combining(&self, virt: u64) -> bool {
        self.leaf(virt).is_some_and(|e| {
            e & (crate::arch::x86_64::pat::PTE_PAT | CACHE_DISABLE | WRITE_THROUGH)
                == crate::arch::x86_64::pat::PTE_PAT
        })
    }

    /// Removes the shared-memory mapping that starts at `virt` and returns
    /// the object.
    pub fn unmap_shared(&self, virt: u64) -> Result<SharedMemory, MapError> {
        let memory = {
            let mut shared = self.shared.lock();
            let at = shared
                .iter()
                .position(|(start, _)| *start == virt)
                .ok_or(MapError::NotMapped)?;
            shared.swap_remove(at).1
        };
        let _guard = self.lock.lock();
        for i in 0..memory.pages() {
            let _ = self.clear_leaf(virt + i as u64 * 4096, true);
        }
        Ok(memory)
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
        // Pages shared copy-on-write get their own frames first (P13 K6).
        if !data.is_empty() {
            let end = virt.checked_add(data.len() as u64).ok_or(BadAddress)?;
            let mut page = page_of(virt);
            while page < end {
                if self.leaf(page).is_some_and(|e| e & COW != 0) {
                    self.unshare(page, need_write);
                }
                page += 4096;
            }
        }
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

    /// A copy of the user half for `fork` (`docs/linux.md`): P13 K6, every
    /// page the address space owns is shared copy-on-write (both entries
    /// read-only with [`COW`], the frame's count up by one; the first
    /// write copies it); shared-memory and device pages are mapped again
    /// (the same frames) and the objects cloned.
    pub fn clone_user(&self) -> Result<AddressSpace, MapError> {
        let copy = AddressSpace::new()?;
        let _guard = self.lock.lock();
        let _copy_guard = copy.lock.lock();
        // SAFETY: this address space's user tables, edited under its lock.
        let result = unsafe { self.clone_level(&copy, self.root, 4, 0) };
        // Our entries became read-only: the TLB must forget the old ones
        // (this process's threads all run on this processor).
        if self.is_active() {
            // SAFETY: reloading the same root flushes the user half's TLB
            // entries; the kernel half is unchanged.
            unsafe { paging::load_root(self.root) };
        }
        result?;
        drop(_copy_guard);
        drop(_guard);
        *copy.shared.lock() = self.shared.lock().clone();
        *copy.devices.lock() = self.devices.lock().clone();
        Ok(copy)
    }

    /// Copies the leaves under the table `phys` (level `level`, covering
    /// addresses from `base`) into `copy`.
    ///
    /// # Safety
    ///
    /// `phys` must be a table of this address space's user half, and both
    /// locks held.
    unsafe fn clone_level(
        &self,
        copy: &AddressSpace,
        phys: u64,
        level: u32,
        base: u64,
    ) -> Result<(), MapError> {
        let entries = if level == 4 { 0..256 } else { 0..512 };
        let span = 1u64 << (12 + 9 * (level - 1));
        for i in entries {
            // SAFETY: the caller's guarantee; read only.
            let entry = unsafe { table(phys) }[i];
            if entry & PRESENT == 0 {
                continue;
            }
            let virt = base + i as u64 * span;
            if level > 1 {
                // SAFETY: a lower table of the same hierarchy.
                unsafe { self.clone_level(copy, entry & ADDR_MASK, level - 1, virt)? };
                continue;
            }
            let leaf = if entry & SHARED != 0 {
                entry
            } else {
                // Copy-on-write: read-only in both, writable again after
                // the copy if it was.
                let was = if entry & (WRITABLE | WAS_WRITABLE) != 0 {
                    WAS_WRITABLE
                } else {
                    0
                };
                let shared = (entry & !WRITABLE) | COW | was;
                // SAFETY: the caller's guarantee (our table, our lock).
                let ours = unsafe { table(phys) };
                ours[i] = shared;
                super::cow::COUNTS.lock().share(entry & ADDR_MASK);
                shared
            };
            if let Err(e) = copy.set_leaf(virt, leaf) {
                if entry & SHARED == 0 {
                    pmm::free_frame(PhysFrame::from_addr(leaf & ADDR_MASK));
                }
                return Err(e);
            }
        }
        Ok(())
    }

    /// Exchanges the user halves of `self` and `other` (with their
    /// shared-memory and device mappings), for `execve`: the new program
    /// is built in `other`, swapped in, and the old one leaves with
    /// `other`. The TLB is flushed if `self` is active.
    pub fn swap_user(&self, other: &AddressSpace) {
        let _irq = crate::sync::IrqGuard::new();
        {
            let _a = self.lock.lock();
            let _b = other.lock.lock();
            // SAFETY: two distinct PML4s (each owned by its address
            // space), edited under both locks; only the user slots move.
            unsafe {
                let mut mine = [0u64; 256];
                mine.copy_from_slice(&table(self.root)[..256]);
                let theirs = &mut table(other.root)[..256];
                let saved: [u64; 256] = {
                    let mut t = [0u64; 256];
                    t.copy_from_slice(theirs);
                    t
                };
                theirs.copy_from_slice(&mine);
                table(self.root)[..256].copy_from_slice(&saved);
            }
        }
        core::mem::swap(&mut *self.shared.lock(), &mut *other.shared.lock());
        core::mem::swap(&mut *self.devices.lock(), &mut *other.devices.lock());
        for space in [self, other] {
            if space.is_active() {
                // SAFETY: reloading the same root flushes the user
                // half's TLB entries; the kernel half is unchanged.
                unsafe { paging::load_root(space.root) };
            }
        }
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

/// Switches this processor to the kernel's own page tables (if a
/// process's are loaded): kernel threads and idle run there, so that no
/// processor keeps a process's tables after it ran its last thread
/// (`docs/smp.md`).
pub fn activate_kernel() {
    if paging::root() != paging::kernel_root() {
        // SAFETY: the kernel's PML4 maps everything the kernel uses.
        unsafe { paging::load_root(paging::kernel_root()) };
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
        if level == 1 && entry & SHARED != 0 {
            // Borrowed from a SharedMemory, which frees it.
            continue;
        }
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
