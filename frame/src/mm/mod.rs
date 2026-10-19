// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Memory management: physical frames, kernel page tables, kernel heap.
//!
//! ```text
//!  virtual address                      what
//!  0xFFFF_8000_0000_0000 ...            HHDM: physical 0.. mapped 1:1 (2 MiB pages)
//!  0xFFFF_FF00_0000_0000 ... +512 GiB   dynamic region: heap, then other mappings
//!  0xFFFF_FFFF_8000_0000 ...            kernel image (linker.ld)
//! ```

pub mod bitmap;
pub mod frames;
#[cfg(target_os = "none")]
pub mod heap;
#[cfg(target_os = "none")]
pub mod paging;
#[cfg(target_os = "none")]
pub mod pmm;

/// Start of the region where the kernel creates its own mappings (PML4 slot
/// 510).
pub const DYNAMIC_BASE: u64 = 0xFFFF_FF00_0000_0000;
/// Size of the dynamic region: one PML4 entry.
pub const DYNAMIC_SIZE: u64 = 512 << 30;
/// The kernel heap occupies the start of the dynamic region.
pub const HEAP_BASE: u64 = DYNAMIC_BASE;
/// Initial (and for now fixed) heap size.
pub const HEAP_SIZE: u64 = 4 << 20;

/// Sets up the frame allocator, takes over the page tables and creates the
/// heap.
#[cfg(target_os = "none")]
pub(crate) fn init(info: &'static bootinfo::BootInfo) {
    pmm::init(info);
    paging::init(info);
    heap::init();
}
