// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The kernel heap: `HEAP_SIZE` bytes at `HEAP_BASE`, backed by frames from
//! the PMM, managed by `linked_list_allocator::Heap` (MIT/Apache-2.0) under
//! a [`SpinLock`]. It is the `#[global_allocator]`, so `alloc::vec::Vec`,
//! `alloc::string::String` and friends work in the kernel.

use super::paging::{self, Flags};
use super::{HEAP_BASE, HEAP_SIZE, pmm};
use crate::sync::SpinLock;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use linked_list_allocator::Heap;

struct KernelHeap(SpinLock<Heap>);

// SAFETY: allocate_first_fit returns blocks that fit `layout` and do not
// overlap live blocks; deallocate is given only pointers that alloc returned
// for the same layout (GlobalAlloc's contract on the caller).
unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0
            .lock()
            .allocate_first_fit(layout)
            .map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(ptr) = NonNull::new(ptr) {
            // SAFETY: the caller passes a block this allocator returned for
            // `layout`.
            unsafe { self.0.lock().deallocate(ptr, layout) };
        }
    }
}

#[global_allocator]
static HEAP: KernelHeap = KernelHeap(SpinLock::new(Heap::empty()));

/// Maps the heap's pages and hands them to the allocator.
pub(crate) fn init() {
    let mut page = HEAP_BASE;
    while page < HEAP_BASE + HEAP_SIZE {
        let Some(frame) = pmm::alloc_frame() else {
            panic!("heap: out of physical memory");
        };
        if let Err(e) = paging::map(page, frame, Flags::DATA) {
            panic!("heap: cannot map {page:#x}: {e:?}");
        }
        page += 4096;
    }
    let mut heap = HEAP.0.lock();
    assert!(heap.size() == 0, "heap: initialised twice");
    // SAFETY: HEAP_BASE..HEAP_BASE + HEAP_SIZE was just mapped writable to
    // frames owned by these mappings, nothing else refers to it, and the
    // heap is initialised only once (checked above).
    unsafe { heap.init(HEAP_BASE as *mut u8, HEAP_SIZE as usize) };
}

/// Heap usage in bytes.
#[derive(Clone, Copy, Debug)]
pub struct HeapStats {
    /// Total size.
    pub size: usize,
    /// Bytes in allocated blocks.
    pub used: usize,
    /// Bytes available.
    pub free: usize,
}

/// Current heap usage.
pub fn stats() -> HeapStats {
    let heap = HEAP.0.lock();
    HeapStats {
        size: heap.size(),
        used: heap.used(),
        free: heap.free(),
    }
}
