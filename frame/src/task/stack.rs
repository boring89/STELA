// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Kernel stacks: 32 KiB of frames from the PMM, mapped in slots of the
//! dynamic region with an unmapped guard below each one.
//!
//! ```text
//!  STACK_REGION + slot * 64 KiB
//!  ┌──────────────────────┬──────────────────────┐
//!  │ guard (32 KiB,       │ stack (32 KiB,       │ ◄─ top
//!  │ not mapped)          │ 8 frames)            │
//!  └──────────────────────┴──────────────────────┘
//! ```
//!
//! Running off the bottom of a stack touches the guard and page faults;
//! the fault cannot be pushed on the same stack, so it turns into a double
//! fault, which has its own stack (IST1) and reports the panic.

use crate::mm::paging::{self, Flags};
use crate::mm::{DYNAMIC_BASE, pmm};
use crate::sync::SpinLock;
use alloc::vec::Vec;

/// Size of a kernel stack.
pub const KERNEL_STACK_SIZE: u64 = 32 * 1024;
/// Virtual space per stack: the stack and its guard.
const SLOT_SIZE: u64 = 2 * KERNEL_STACK_SIZE;
/// Start of the stack slots: 64 GiB into the dynamic region, far from the
/// heap.
const STACK_REGION: u64 = DYNAMIC_BASE + (64 << 30);
/// Number of slots (1 GiB of address space).
const MAX_SLOTS: u32 = 16384;

/// Slots handed out so far, and returned ones for reuse.
struct Slots {
    next: u32,
    free: Vec<u32>,
}

static SLOTS: SpinLock<Slots> = SpinLock::new(Slots {
    next: 0,
    free: Vec::new(),
});

/// A mapped kernel stack; unmapped and freed when dropped.
#[derive(Debug)]
pub struct KernelStack {
    slot: u32,
}

impl KernelStack {
    /// Allocates and maps a stack. `None` when frames or slots run out.
    pub fn new() -> Option<KernelStack> {
        let slot = {
            let mut slots = SLOTS.lock();
            match slots.free.pop() {
                Some(slot) => slot,
                None if slots.next < MAX_SLOTS => {
                    slots.next += 1;
                    slots.next - 1
                }
                None => return None,
            }
        };
        let stack = KernelStack { slot };
        let bottom = stack.bottom();
        for page in 0..KERNEL_STACK_SIZE / 4096 {
            let virt = bottom + page * 4096;
            // On failure, dropping `stack` unmaps the pages mapped so far
            // and returns the slot.
            pmm::alloc_frame().and_then(|frame| paging::map(virt, frame, Flags::DATA).ok())?;
        }
        Some(stack)
    }

    /// The lowest mapped address.
    fn bottom(&self) -> u64 {
        STACK_REGION + u64::from(self.slot) * SLOT_SIZE + KERNEL_STACK_SIZE
    }

    /// One past the highest address (the initial stack pointer).
    pub fn top(&self) -> u64 {
        self.bottom() + KERNEL_STACK_SIZE
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        let bottom = self.bottom();
        for page in 0..KERNEL_STACK_SIZE / 4096 {
            if let Ok(frame) = paging::unmap(bottom + page * 4096) {
                pmm::free_frame(frame);
            }
        }
        SLOTS.lock().free.push(self.slot);
    }
}
