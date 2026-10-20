// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The physical frame allocator's logic: a [`Bitmap`] with one bit per 4 KiB
//! frame, built from the boot memory map. Pure logic, tested on the host;
//! `pmm.rs` places the bitmap in memory and makes it global.

use super::bitmap::Bitmap;
use bootinfo::{MemoryKind, MemoryRegion, PAGE_SIZE};

/// One past the highest usable byte: the bitmap needs to cover no more,
/// since frames above it can never be handed out.
pub fn usable_top(regions: &[MemoryRegion]) -> u64 {
    regions
        .iter()
        .filter(|r| r.kind() == Some(MemoryKind::Usable))
        .map(MemoryRegion::phys_end)
        .max()
        .unwrap_or(0)
}

/// Where to put `bytes` of bitmap: the start of the largest usable region
/// (skipping frame 0, which is never handed out). `None` if it does not fit.
pub fn bitmap_placement(regions: &[MemoryRegion], bytes: u64) -> Option<u64> {
    let largest = regions
        .iter()
        .filter(|r| r.kind() == Some(MemoryKind::Usable))
        .max_by_key(|r| r.page_count)?;
    let start = largest.phys_start.max(PAGE_SIZE);
    (start + bytes <= largest.phys_end()).then_some(start)
}

/// Whether any byte of `start..end` lies in a region of one of `kinds`.
pub fn overlaps(regions: &[MemoryRegion], start: u64, end: u64, kinds: &[MemoryKind]) -> bool {
    regions.iter().any(|r| {
        r.kind().is_some_and(|k| kinds.contains(&k)) && r.phys_start < end && start < r.phys_end()
    })
}

/// Whether any byte of `start..end` is RAM according to the memory map
/// (usable, kernel/boot, ACPI tables or ACPI NVS). Device mappings must not
/// alias RAM: writes through them would bypass Rust's ownership of it.
pub fn overlaps_ram(regions: &[MemoryRegion], start: u64, end: u64) -> bool {
    overlaps(
        regions,
        start,
        end,
        &[
            MemoryKind::Usable,
            MemoryKind::KernelAndBoot,
            MemoryKind::AcpiReclaimable,
            MemoryKind::AcpiNvs,
        ],
    )
}

/// Hands out 4 KiB physical frames.
pub struct FrameAllocator<'a> {
    map: Bitmap<'a>,
    /// Where the next search starts (every frame below it is used).
    next: usize,
    free: usize,
}

impl<'a> FrameAllocator<'a> {
    /// Builds the allocator: frames in usable regions are free, except frame
    /// 0 and the ranges in `reserved` (`(start, end)` physical byte
    /// addresses, such as the bitmap itself).
    ///
    /// `storage` must hold at least
    /// `Bitmap::words_for(usable_top(regions) / PAGE_SIZE)` words.
    pub fn new(
        storage: &'a mut [u64],
        regions: &[MemoryRegion],
        reserved: &[(u64, u64)],
    ) -> FrameAllocator<'a> {
        let frames = (usable_top(regions) / PAGE_SIZE) as usize;
        let mut map = Bitmap::new_full(storage, frames);
        for region in regions {
            if region.kind() == Some(MemoryKind::Usable) {
                map.clear_range(
                    (region.phys_start / PAGE_SIZE) as usize,
                    (region.phys_end() / PAGE_SIZE) as usize,
                );
            }
        }
        for &(start, end) in reserved {
            map.set_range(
                (start / PAGE_SIZE) as usize,
                end.div_ceil(PAGE_SIZE) as usize,
            );
        }
        map.set(0);
        let free = map.count_clear();
        FrameAllocator { map, next: 0, free }
    }

    /// Number of free frames.
    pub const fn free_frames(&self) -> usize {
        self.free
    }

    /// Number of frames the bitmap covers.
    pub const fn total_frames(&self) -> usize {
        self.map.len()
    }

    /// Allocates one frame; returns its physical address.
    pub fn alloc(&mut self) -> Option<u64> {
        let index = self.map.find_clear(self.next)?;
        self.map.set(index);
        self.next = index + 1;
        self.free -= 1;
        Some(index as u64 * PAGE_SIZE)
    }

    /// Allocates `count` physically contiguous frames (first fit); returns
    /// the address of the first.
    pub fn alloc_contiguous(&mut self, count: usize) -> Option<u64> {
        let start = self.map.find_clear_run(count, 1)?;
        self.map.set_range(start, start + count);
        self.free -= count;
        if start <= self.next {
            self.next = self.map.find_clear(self.next).unwrap_or(self.map.len());
        }
        Some(start as u64 * PAGE_SIZE)
    }

    /// Returns a frame. Panics on a misaligned address, a frame outside the
    /// bitmap or a frame that is already free (a double free).
    pub fn free(&mut self, phys: u64) {
        assert!(
            phys.is_multiple_of(PAGE_SIZE),
            "free of unaligned frame {phys:#x}"
        );
        let index = (phys / PAGE_SIZE) as usize;
        assert!(
            index < self.map.len() && self.map.is_set(index),
            "free of a frame that is not allocated: {phys:#x}"
        );
        self.map.clear(index);
        self.free += 1;
        self.next = self.next.min(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_overlap() {
        let regions = [
            MemoryRegion::new(MemoryKind::Usable, 0x1000, 4),
            MemoryRegion::new(MemoryKind::Reserved, 0x5000, 1),
            MemoryRegion::new(MemoryKind::AcpiNvs, 0x8000, 1),
        ];
        assert!(overlaps_ram(&regions, 0x4000, 0x5000));
        assert!(!overlaps_ram(&regions, 0x5000, 0x6000));
        assert!(!overlaps_ram(&regions, 0x6000, 0x8000));
        assert!(overlaps_ram(&regions, 0x7FFF, 0x8001));
        assert!(!overlaps_ram(&regions, 0xFEE0_0000, 0xFEE0_1000));
    }

    fn regions() -> [MemoryRegion; 4] {
        [
            MemoryRegion::new(MemoryKind::Usable, 0, 8), // frames 0-7
            MemoryRegion::new(MemoryKind::Reserved, 0x8000, 8), // 8-15
            MemoryRegion::new(MemoryKind::Usable, 0x10000, 32), // 16-47
            MemoryRegion::new(MemoryKind::KernelAndBoot, 0x30000, 4), // 48-51
        ]
    }

    #[test]
    fn top_and_placement() {
        let r = regions();
        assert_eq!(usable_top(&r), 0x30000);
        assert_eq!(bitmap_placement(&r, 4096), Some(0x10000));
        assert_eq!(bitmap_placement(&r, 32 * 4096), Some(0x10000));
        assert_eq!(bitmap_placement(&r, 33 * 4096), None);
        assert_eq!(bitmap_placement(&[], 1), None);
        // Frame 0 is skipped.
        let low = [MemoryRegion::new(MemoryKind::Usable, 0, 4)];
        assert_eq!(bitmap_placement(&low, 4096), Some(0x1000));
    }

    #[test]
    fn builds_from_regions() {
        let mut words = [0; 1];
        let a = FrameAllocator::new(&mut words, &regions(), &[(0x10000, 0x11000)]);
        assert_eq!(a.total_frames(), 48);
        // 8 + 32 usable, minus frame 0, minus the reserved frame 16.
        assert_eq!(a.free_frames(), 38);
    }

    #[test]
    fn alloc_and_free() {
        let mut words = [0; 1];
        let mut a = FrameAllocator::new(&mut words, &regions(), &[(0x10000, 0x10001)]);
        let first = a.alloc().unwrap();
        assert_eq!(first, 0x1000);
        let all: Vec<u64> = core::iter::from_fn(|| a.alloc()).collect();
        assert_eq!(all.len(), 37);
        assert!(
            all.iter()
                .all(|&f| f != 0 && !(0x8000..0x11000).contains(&f))
        );
        assert_eq!(a.free_frames(), 0);
        a.free(0x5000);
        assert_eq!(a.alloc(), Some(0x5000));
        a.free(first);
        assert_eq!(a.free_frames(), 1);
    }

    #[test]
    fn contiguous() {
        let mut words = [0; 1];
        let mut a = FrameAllocator::new(&mut words, &regions(), &[]);
        assert_eq!(a.alloc_contiguous(8), Some(0x10000)); // 1-7 too short
        assert_eq!(a.alloc(), Some(0x1000));
        assert_eq!(a.alloc_contiguous(24), Some(0x18000));
        assert_eq!(a.alloc_contiguous(7), None);
        assert_eq!(a.alloc_contiguous(6), Some(0x2000));
        assert_eq!(a.free_frames(), 0);
    }

    #[test]
    #[should_panic(expected = "not allocated")]
    fn double_free_panics() {
        let mut words = [0; 1];
        let mut a = FrameAllocator::new(&mut words, &regions(), &[]);
        let frame = a.alloc().unwrap();
        a.free(frame);
        a.free(frame);
    }
}
