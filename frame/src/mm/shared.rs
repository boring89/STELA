// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared memory: a set of zeroed frames that several address spaces can
//! map at once ([`super::user::AddressSpace::map_shared`]). The frames go
//! back to the PMM when the last [`SharedMemory`] clone is dropped; every
//! address space that maps them holds a clone, so they cannot be freed
//! while mapped.

use super::pmm::{self, PhysFrame};
use alloc::sync::Arc;
use alloc::vec::Vec;
use bootinfo::HHDM_BASE;

struct Frames(Vec<PhysFrame>);

impl Drop for Frames {
    fn drop(&mut self) {
        for frame in self.0.drain(..) {
            pmm::free_frame(frame);
        }
    }
}

/// Frames that can be mapped into user address spaces; cloning shares
/// them.
#[derive(Clone)]
pub struct SharedMemory {
    frames: Arc<Frames>,
}

impl core::fmt::Debug for SharedMemory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SharedMemory({} pages)", self.pages())
    }
}

/// An offset and length outside the memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfRange;

impl SharedMemory {
    /// `pages` zeroed frames (not contiguous); `None` when the PMM runs
    /// out (what was taken is given back).
    pub fn new(pages: usize) -> Option<SharedMemory> {
        let mut frames = Frames(Vec::new());
        frames.0.try_reserve_exact(pages).ok()?;
        for _ in 0..pages {
            frames.0.push(pmm::alloc_zeroed_frame()?);
        }
        Some(SharedMemory {
            frames: Arc::new(frames),
        })
    }

    /// `pages` zeroed frames one after another in physical memory, for a
    /// device to reach with DMA (P13 K11, `DeviceDma`); `None` when no run
    /// that long is free.
    pub fn new_contiguous(pages: usize) -> Option<SharedMemory> {
        if pages == 0 {
            return None;
        }
        let run = pmm::alloc_contiguous(pages)?;
        // The run becomes one token per frame (freed one by one).
        let start = run.addr();
        let mut frames = Frames(Vec::new());
        if frames.0.try_reserve_exact(pages).is_err() {
            for i in 0..pages as u64 {
                pmm::free_frame(PhysFrame::from_addr(start + i * 4096));
            }
            return None;
        }
        for i in 0..pages as u64 {
            frames.0.push(PhysFrame::from_addr(start + i * 4096));
        }
        let memory = SharedMemory {
            frames: Arc::new(frames),
        };
        let zero = alloc::vec![0u8; 4096];
        for i in 0..pages as u64 {
            let _ = memory.write(i * 4096, &zero);
        }
        Some(memory)
    }

    /// The physical address of the first frame (of all of them, for memory
    /// made by [`SharedMemory::new_contiguous`]).
    pub fn phys(&self) -> u64 {
        self.frame_addr(0)
    }

    /// Number of pages.
    pub fn pages(&self) -> usize {
        self.frames.0.len()
    }

    /// Size in bytes.
    pub fn size(&self) -> u64 {
        self.pages() as u64 * 4096
    }

    /// Physical address of page `i`.
    pub(crate) fn frame_addr(&self, i: usize) -> u64 {
        self.frames.0[i].addr()
    }

    /// Whether `other` shares the same frames.
    pub fn same_as(&self, other: &SharedMemory) -> bool {
        Arc::ptr_eq(&self.frames, &other.frames)
    }

    /// Calls `f(physical address, offset in the buffer, length)` for each
    /// page-sized piece of `offset..offset + len`.
    fn pieces(
        &self,
        offset: u64,
        len: usize,
        mut f: impl FnMut(u64, usize, usize),
    ) -> Result<(), OutOfRange> {
        let end = offset.checked_add(len as u64).ok_or(OutOfRange)?;
        if end > self.size() {
            return Err(OutOfRange);
        }
        let mut at = offset;
        while at < end {
            let piece = ((at & !0xFFF) + 4096).min(end) - at;
            let phys = self.frame_addr((at / 4096) as usize) + (at & 0xFFF);
            f(phys, (at - offset) as usize, piece as usize);
            at += piece;
        }
        Ok(())
    }

    /// Copies bytes at `offset` into `buf`.
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> Result<(), OutOfRange> {
        self.pieces(offset, buf.len(), |phys, at, len| {
            // SAFETY: `phys..phys + len` lies in one frame owned by this
            // object, reached through the HHDM. User threads may write it
            // at the same time; the bytes are copied as plain data.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (HHDM_BASE + phys) as *const u8,
                    buf[at..].as_mut_ptr(),
                    len,
                )
            };
        })
    }

    /// Copies `data` to `offset`.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<(), OutOfRange> {
        self.pieces(offset, data.len(), |phys, at, len| {
            // SAFETY: as in `read`; no Rust reference points into the frame.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data[at..].as_ptr(),
                    (HHDM_BASE + phys) as *mut u8,
                    len,
                )
            };
        })
    }
}
