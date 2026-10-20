// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The program heap (`docs/userland.md`): `#[global_allocator]` over
//! shared-memory objects. The first use creates a 1 MiB `Memory` object
//! and maps it; when nothing fits, another one (at least 1 MiB, or as big
//! as the request) is added. Blocks are kept in one free list sorted by
//! address and merged with their neighbours when freed (first fit).
//!
//! ```text
//!  free block:       [size | next][ ...                     ]
//!  allocated block:  [ ... padding ...][start | size][ user data ... ]
//!                                       ^ 16-byte header just before the pointer
//! ```

use crate::sync::SpinMutex;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

const HEADER: usize = 16;
const MIN_BLOCK: usize = 32;
/// The size of the first chunk and the smallest later one.
pub const CHUNK: usize = 1 << 20;

/// A free block's header (in the block itself).
#[repr(C)]
struct Free {
    size: usize,
    next: usize,
}

/// Where the free list starts (0: empty) and how much was mapped.
struct State {
    head: usize,
    mapped: usize,
}

/// The allocator.
pub struct Heap {
    state: SpinMutex<State>,
}

/// Bytes of memory the heap has mapped so far.
pub fn mapped() -> usize {
    HEAP.state.lock().mapped
}

impl Heap {
    const fn new() -> Heap {
        Heap {
            state: SpinMutex::new(State { head: 0, mapped: 0 }),
        }
    }
}

/// Reads the free block header at `at`.
///
/// # Safety
///
/// `at` must be the start of a free block of this heap.
unsafe fn free_at(at: usize) -> Free {
    // SAFETY: the caller's guarantee; free blocks are at least 16-byte
    // aligned and hold a header.
    unsafe { ptr::read(at as *const Free) }
}

/// Writes a free block header at `at`.
///
/// # Safety
///
/// `at..at + size` must be memory of this heap that nothing else uses.
unsafe fn set_free(at: usize, size: usize, next: usize) {
    // SAFETY: the caller's guarantee.
    unsafe { ptr::write(at as *mut Free, Free { size, next }) };
}

impl State {
    /// Puts `start..start + size` into the free list, merged with the
    /// blocks right before and after it.
    ///
    /// # Safety
    ///
    /// The range must be heap memory that nothing uses.
    unsafe fn insert(&mut self, start: usize, size: usize) {
        let mut prev = 0usize;
        let mut cur = self.head;
        while cur != 0 && cur < start {
            prev = cur;
            // SAFETY: list entries are free blocks.
            cur = unsafe { free_at(cur) }.next;
        }
        let (mut start, mut size) = (start, size);
        // Merge with the next block.
        if cur != 0 && start + size == cur {
            // SAFETY: cur is a free block.
            let next = unsafe { free_at(cur) };
            size += next.size;
            cur = next.next;
        }
        // Merge with the previous block.
        if prev != 0 {
            // SAFETY: prev is a free block.
            let p = unsafe { free_at(prev) };
            if prev + p.size == start {
                start = prev;
                size += p.size;
                // SAFETY: the merged range is free heap memory.
                unsafe { set_free(start, size, cur) };
                return;
            }
            // SAFETY: as above; prev now points to the new block.
            unsafe {
                set_free(start, size, cur);
                set_free(prev, p.size, start);
            }
            return;
        }
        // SAFETY: the range is free heap memory.
        unsafe { set_free(start, size, cur) };
        self.head = start;
    }

    /// Takes a block for `layout` from the free list.
    fn take(&mut self, layout: Layout) -> Option<usize> {
        let align = layout.align().max(HEADER);
        let size = layout.size().max(1).next_multiple_of(HEADER);
        let mut prev = 0usize;
        let mut cur = self.head;
        while cur != 0 {
            // SAFETY: list entries are free blocks.
            let block = unsafe { free_at(cur) };
            let user = (cur + HEADER).next_multiple_of(align);
            let end = user + size;
            if end <= cur + block.size {
                // Split off the tail if it is big enough to be a block.
                let mut used = (end - cur).next_multiple_of(HEADER);
                let rest = block.size - used;
                let next = if rest >= MIN_BLOCK {
                    // SAFETY: the tail is part of this free block.
                    unsafe { set_free(cur + used, rest, block.next) };
                    cur + used
                } else {
                    used = block.size;
                    block.next
                };
                if prev == 0 {
                    self.head = next;
                } else {
                    // SAFETY: prev is a free block.
                    let p = unsafe { free_at(prev) };
                    // SAFETY: as above.
                    unsafe { set_free(prev, p.size, next) };
                }
                // SAFETY: the header slot is inside the block, before user.
                unsafe { ptr::write((user - HEADER) as *mut [usize; 2], [cur, used]) };
                return Some(user);
            }
            prev = cur;
            cur = block.next;
        }
        None
    }

    /// Maps another chunk big enough for `layout`.
    fn grow(&mut self, layout: Layout) -> bool {
        let need = (layout.size() + layout.align() + 2 * HEADER).next_multiple_of(4096);
        let size = need.max(CHUNK);
        let Ok(memory) = crate::ipc::memory_create(size as u64) else {
            return false;
        };
        let Ok(addr) = crate::ipc::memory_map(memory, true) else {
            let _ = crate::ipc::handle_close(memory);
            return false;
        };
        // The mapping keeps the memory; the handle is not needed.
        let _ = crate::ipc::handle_close(memory);
        self.mapped += size;
        // SAFETY: the new mapping is ours alone.
        unsafe { self.insert(addr as usize, size) };
        true
    }
}

// SAFETY: blocks come from mappings only this allocator uses, are aligned
// as asked (headers keep their start and size), are never handed out twice
// (the free list is changed under the lock), and dealloc takes back only
// pointers alloc returned.
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut state = self.state.lock();
        if let Some(p) = state.take(layout) {
            return p as *mut u8;
        }
        if state.grow(layout)
            && let Some(p) = state.take(layout)
        {
            return p as *mut u8;
        }
        ptr::null_mut()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: alloc wrote the block's start and size just before ptr.
        let [start, size] = unsafe { ptr::read((ptr as usize - HEADER) as *const [usize; 2]) };
        let mut state = self.state.lock();
        // SAFETY: the block was handed out by alloc and is given back.
        unsafe { state.insert(start, size) };
    }
}

#[cfg_attr(target_os = "none", global_allocator)]
static HEAP: Heap = Heap::new();
