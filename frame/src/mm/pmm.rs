// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The global physical memory manager (PMM): a [`FrameAllocator`] whose
//! bitmap lives at the start of the largest usable region, reached through
//! the HHDM.
//!
//! Frames are handed out as [`PhysFrame`] tokens that cannot be copied or
//! forged, so safe code can only free a frame it was given.

use super::bitmap::Bitmap;
use super::frames::{FrameAllocator, bitmap_placement, usable_top};
use crate::sync::SpinLock;
use bootinfo::{BootInfo, HHDM_BASE, PAGE_SIZE};

/// One 4 KiB physical frame owned by its holder.
#[derive(Debug, PartialEq, Eq)]
pub struct PhysFrame(u64);

impl PhysFrame {
    /// Physical address of the frame.
    pub const fn addr(&self) -> u64 {
        self.0
    }

    /// Re-creates a token for a frame taken out of a page table.
    pub(crate) const fn from_addr(addr: u64) -> PhysFrame {
        PhysFrame(addr)
    }
}

/// Physically contiguous frames owned by their holder.
#[derive(Debug, PartialEq, Eq)]
pub struct PhysFrames {
    start: u64,
    count: usize,
}

impl PhysFrames {
    /// Physical address of the first frame.
    pub const fn addr(&self) -> u64 {
        self.start
    }

    /// Number of frames.
    pub const fn count(&self) -> usize {
        self.count
    }
}

static PMM: SpinLock<Option<FrameAllocator<'static>>> = SpinLock::new(None);

/// Builds the frame allocator from the boot memory map.
pub(crate) fn init(info: &'static BootInfo) {
    let regions = info.regions();
    let frames = (usable_top(regions) / PAGE_SIZE) as usize;
    let words = Bitmap::words_for(frames);
    let bytes = (words * 8) as u64;
    let Some(phys) = bitmap_placement(regions, bytes) else {
        panic!("pmm: no usable region can hold the {bytes}-byte frame bitmap");
    };
    // SAFETY: `phys..phys + bytes` lies inside a usable region (checked by
    // bitmap_placement), so nothing else uses it; the HHDM maps all of it
    // writable; the range is marked used below, so the allocator never hands
    // it out; and this runs once (PMM is still None), so this is the only
    // reference. u64 needs 8-byte alignment; `phys` is page aligned.
    let storage: &'static mut [u64] =
        unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + phys) as *mut u64, words) };
    let allocator = FrameAllocator::new(storage, regions, &[(phys, phys + bytes)]);
    let mut pmm = PMM.lock();
    assert!(pmm.is_none(), "pmm: initialised twice");
    *pmm = Some(allocator);
}

/// Allocates one frame (contents undefined).
pub fn alloc_frame() -> Option<PhysFrame> {
    PMM.lock().as_mut()?.alloc().map(PhysFrame)
}

/// Allocates one frame filled with zeros.
pub fn alloc_zeroed_frame() -> Option<PhysFrame> {
    let frame = alloc_frame()?;
    // SAFETY: the frame was just allocated, so this is its only user.
    unsafe { zero(frame.addr(), 1) };
    Some(frame)
}

/// Returns a frame.
pub fn free_frame(frame: PhysFrame) {
    if let Some(pmm) = PMM.lock().as_mut() {
        pmm.free(frame.0);
    }
}

/// Allocates `count` physically contiguous frames (contents undefined).
pub fn alloc_contiguous(count: usize) -> Option<PhysFrames> {
    let start = PMM.lock().as_mut()?.alloc_contiguous(count)?;
    Some(PhysFrames { start, count })
}

/// Returns contiguous frames.
pub fn free_contiguous(frames: PhysFrames) {
    if let Some(pmm) = PMM.lock().as_mut() {
        for i in 0..frames.count as u64 {
            pmm.free(frames.start + i * PAGE_SIZE);
        }
    }
}

/// `(free frames, frames covered by the bitmap)`.
pub fn stats() -> (usize, usize) {
    PMM.lock()
        .as_ref()
        .map_or((0, 0), |p| (p.free_frames(), p.total_frames()))
}

/// Fills `count` frames starting at `phys` with zeros through the HHDM.
///
/// # Safety
///
/// The caller must own the frames, with no reference to their contents
/// alive.
unsafe fn zero(phys: u64, count: usize) {
    // SAFETY: the caller owns the frames and holds no reference into them;
    // the HHDM maps all RAM writable.
    unsafe { core::ptr::write_bytes((HHDM_BASE + phys) as *mut u8, 0, count * PAGE_SIZE as usize) };
}
