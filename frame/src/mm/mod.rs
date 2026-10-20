// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Memory management: physical frames, kernel page tables, kernel heap.
//!
//! ```text
//!  virtual address                      what
//!  0x0000_0000_0000_0000 ... 128 TiB    user half: per address space (user.rs)
//!  0xFFFF_8000_0000_0000 ...            HHDM: physical 0.. mapped 1:1 (2 MiB pages)
//!  0xFFFF_FF00_0000_0000 ... +256 GiB   dynamic region: heap, then other mappings
//!  0xFFFF_FF40_0000_0000 ... +256 GiB   dynamic region, MMIO window (uncached)
//!  0xFFFF_FFFF_8000_0000 ...            kernel image (linker.ld)
//! ```

pub mod bitmap;
pub mod frames;
#[cfg(target_os = "none")]
pub mod heap;
#[cfg(target_os = "none")]
pub mod mmio;
#[cfg(target_os = "none")]
pub mod paging;
#[cfg(target_os = "none")]
pub mod pmm;
#[cfg(target_os = "none")]
pub mod shared;
#[cfg(target_os = "none")]
pub mod user;

/// Start of the region where the kernel creates its own mappings (PML4 slot
/// 510).
pub const DYNAMIC_BASE: u64 = 0xFFFF_FF00_0000_0000;
/// Size of the dynamic region: one PML4 entry.
pub const DYNAMIC_SIZE: u64 = 512 << 30;
/// The upper half of the dynamic region holds device (MMIO) mappings.
pub const MMIO_BASE: u64 = DYNAMIC_BASE + DYNAMIC_SIZE / 2;
/// Size of the MMIO window.
pub const MMIO_SIZE: u64 = DYNAMIC_SIZE / 2;
/// The kernel heap occupies the start of the dynamic region.
pub const HEAP_BASE: u64 = DYNAMIC_BASE;
/// Initial (and for now fixed) heap size.
pub const HEAP_SIZE: u64 = 4 << 20;
/// End of the user half of every address space (PML4 slots 0-255): the
/// lowest non-canonical address.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

/// Sets up the frame allocator, takes over the page tables and creates the
/// heap.
#[cfg(target_os = "none")]
pub(crate) fn init(info: &'static bootinfo::BootInfo) {
    pmm::init(info);
    paging::init(info);
    heap::init();
    mmio::init(info);
    init_initrd(info);
}

/// The initial RAM disk, once checked.
#[cfg(target_os = "none")]
static INITRD: crate::sync::Once<&'static [u8]> = crate::sync::Once::new();

/// Checks the loader's initrd range and makes it readable as a slice.
#[cfg(target_os = "none")]
fn init_initrd(info: &'static bootinfo::BootInfo) {
    let (phys, size) = (info.initrd_phys, info.initrd_size);
    if phys == 0 || size == 0 {
        return;
    }
    let Some(end) = phys.checked_add(size) else {
        crate::kprintln!("initrd: bad range {phys:#x} + {size}; ignored");
        return;
    };
    // It must lie in memory the loader allocated (KernelAndBoot), which the
    // frame allocator never hands out, and inside the direct map.
    let inside = info.regions().iter().any(|r| {
        r.kind() == Some(bootinfo::MemoryKind::KernelAndBoot)
            && r.phys_start <= phys
            && end <= r.phys_end()
    });
    if !inside || end > info.hhdm_size {
        crate::kprintln!("initrd: {phys:#x} + {size} is not loader memory; ignored");
        return;
    }
    // SAFETY: the range is KernelAndBoot RAM (checked above): the PMM never
    // allocates it and nothing writes it after the loader, so a shared
    // slice for the rest of time is sound. The HHDM maps it.
    let data = unsafe {
        core::slice::from_raw_parts((bootinfo::HHDM_BASE + phys) as *const u8, size as usize)
    };
    let _ = INITRD.set(data);
}

/// The initial RAM disk the loader read (`\STELA\initrd.tar`), or an
/// empty slice when there is none.
#[cfg(target_os = "none")]
pub fn initrd() -> &'static [u8] {
    INITRD.get().copied().unwrap_or(&[])
}
