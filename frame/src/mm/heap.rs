// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The kernel heap at `HEAP_BASE`, backed by frames from the PMM, managed
//! by `linked_list_allocator::Heap` (MIT/Apache-2.0) under a
//! [`SpinLock`]. It is the `#[global_allocator]`, so `alloc::vec::Vec`,
//! `alloc::string::String` and friends work in the kernel.
//!
//! P12 (`docs/bench.md`): the heap starts at `HEAP_INITIAL` (4 MiB) and
//! grows when an allocation does not fit: at least `GROW_STEP` (1 MiB)
//! more pages are mapped right after the end and handed to the allocator
//! (`Heap::extend`), up to `HEAP_MAX` (64 MiB). It never shrinks.
//!
//! ```text
//!  HEAP_BASE                 mapped end            HEAP_BASE + HEAP_MAX
//!  ├── in use / free ────────┤ (mapped on demand) ──┤
//!                            └── grow: map pages, extend, retry
//! ```

use super::paging::{self, Flags};
use super::{HEAP_BASE, HEAP_INITIAL, HEAP_MAX, pmm};
use crate::sync::SpinLock;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use linked_list_allocator::Heap;

/// The smallest growth step.
pub const GROW_STEP: u64 = 1 << 20;

struct KernelHeap(SpinLock<Heap>);

/// Maps `bytes` (a multiple of 4 KiB) more after the heap's end and gives
/// them to the allocator; `false` past `HEAP_MAX` or without frames (then
/// nothing changes).
fn grow(heap: &mut Heap, bytes: u64) -> bool {
    let top = HEAP_BASE + heap.size() as u64;
    if top + bytes > HEAP_BASE + HEAP_MAX {
        return false;
    }
    let mut page = top;
    while page < top + bytes {
        let mapped =
            pmm::alloc_frame().is_some_and(|frame| paging::map(page, frame, Flags::DATA).is_ok());
        if !mapped {
            // Give back what this attempt mapped.
            let mut back = top;
            while back < page {
                if let Ok(frame) = paging::unmap(back) {
                    pmm::free_frame(frame);
                }
                back += 4096;
            }
            return false;
        }
        page += 4096;
    }
    // SAFETY: top..top + bytes was just mapped writable to frames owned by
    // these mappings, directly after the heap's current end, and nothing
    // else refers to it.
    unsafe { heap.extend(bytes as usize) };
    true
}

// SAFETY: allocate_first_fit returns blocks that fit `layout` and do not
// overlap live blocks; deallocate is given only pointers that alloc returned
// for the same layout (GlobalAlloc's contract on the caller). Growing only
// adds fresh memory after the end.
unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = self.0.lock();
        loop {
            if let Ok(block) = heap.allocate_first_fit(layout) {
                return block.as_ptr();
            }
            // Enough for this request with room for the allocator's own
            // bookkeeping and the alignment.
            let need = (layout.size() as u64 + layout.align() as u64 + 64)
                .next_multiple_of(GROW_STEP)
                .max(GROW_STEP);
            if !grow(&mut heap, need) {
                return ptr::null_mut();
            }
        }
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

/// Maps the heap's first `HEAP_INITIAL` bytes and hands them to the
/// allocator.
pub(crate) fn init() {
    let mut page = HEAP_BASE;
    while page < HEAP_BASE + HEAP_INITIAL {
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
    // SAFETY: HEAP_BASE..HEAP_BASE + HEAP_INITIAL was just mapped writable
    // to frames owned by these mappings, nothing else refers to it, and
    // the heap is initialised only once (checked above).
    unsafe { heap.init(HEAP_BASE as *mut u8, HEAP_INITIAL as usize) };
}

/// Heap usage in bytes.
#[derive(Clone, Copy, Debug)]
pub struct HeapStats {
    /// Mapped size (grows on demand).
    pub size: usize,
    /// Bytes in allocated blocks.
    pub used: usize,
    /// Bytes available without growing.
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
