// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Per-CPU data: each processor has its own [`PerCpu`], found through the
//! GS segment base (MSR IA32_GS_BASE, Intel SDM Vol. 4 Table 2-2; Vol. 3A
//! "Segment Loading Instructions in IA-32e Mode").
//!
//! ```text
//!  IA32_GS_BASE ──► PerCpu { self_ptr, id, apic_id, ticks, ... }
//!                      ▲ │
//!                      └─┘ gs:[0] holds the area's own address, so one
//!                          `mov rax, gs:[0]` yields a normal pointer
//! ```
//!
//! Only the boot processor exists for now (application processors come
//! with SMP start-up, P10), but nothing here assumes a single processor:
//! each one calls [`init_this_cpu`] for itself.
//!
//! The assembly entry paths (system calls) read some fields at fixed
//! offsets from GS; those offsets are the `OFFSET_*` constants, checked at
//! compile time.

use crate::arch::x86_64::msr;
use crate::task::TaskContext;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::offset_of;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// The data of one processor. `self_ptr` must stay the first field.
#[repr(C)]
pub struct PerCpu {
    /// The address of this structure (read through `gs:[0]`).
    self_ptr: u64,
    id: u32,
    apic_id: u32,
    ticks: AtomicU64,
    /// Top of the running thread's kernel stack; the system-call entry
    /// switches to it (`gs:[OFFSET_KERNEL_RSP]`).
    kernel_rsp: AtomicU64,
    /// Where the system-call entry parks the user stack pointer while it
    /// loads the kernel one (`gs:[OFFSET_USER_RSP]`).
    user_rsp_scratch: AtomicU64,
    /// Set when the running thread should give up the processor at the
    /// next opportunity (end of an interrupt).
    need_resched: AtomicBool,
    /// The context of the running thread. Only touched by this processor
    /// with interrupts disabled (see `crate::task`).
    pub(crate) current_task: UnsafeCell<Option<Arc<TaskContext>>>,
    /// The context that switched to the running one, kept alive until the
    /// switch is over (its stack was in use until then).
    pub(crate) previous_task: UnsafeCell<Option<Arc<TaskContext>>>,
}

/// Offset of `kernel_rsp`, used by the system-call entry stub.
pub(crate) const OFFSET_KERNEL_RSP: usize = 24;
/// Offset of `user_rsp_scratch`, used by the system-call entry stub.
pub(crate) const OFFSET_USER_RSP: usize = 32;
const _: () = assert!(offset_of!(PerCpu, kernel_rsp) == OFFSET_KERNEL_RSP);
const _: () = assert!(offset_of!(PerCpu, user_rsp_scratch) == OFFSET_USER_RSP);

impl PerCpu {
    /// Logical processor number (0 = the boot processor).
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The processor's local APIC id.
    pub fn apic_id(&self) -> u32 {
        self.apic_id
    }

    /// Timer interrupts this processor has handled.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    /// Counts one timer interrupt.
    pub(crate) fn tick(&self) {
        self.ticks.fetch_add(1, Ordering::Relaxed);
    }

    /// Records the top of the running thread's kernel stack.
    pub(crate) fn set_kernel_rsp(&self, top: u64) {
        self.kernel_rsp.store(top, Ordering::Relaxed);
    }

    /// Asks for a reschedule at the end of the current interrupt.
    pub fn set_need_resched(&self) {
        self.need_resched.store(true, Ordering::Relaxed);
    }
}

/// Set once the boot processor has its area.
static READY: AtomicBool = AtomicBool::new(false);
/// Number of processors with a per-CPU area.
static ONLINE: AtomicUsize = AtomicUsize::new(0);

/// Creates the calling processor's area and points GS_BASE at it. Each
/// processor calls this once, with interrupts disabled.
pub(crate) fn init_this_cpu(id: u32, apic_id: u32) {
    let area = Box::leak(Box::new(PerCpu {
        self_ptr: 0,
        id,
        apic_id,
        ticks: AtomicU64::new(0),
        kernel_rsp: AtomicU64::new(0),
        user_rsp_scratch: AtomicU64::new(0),
        need_resched: AtomicBool::new(false),
        current_task: UnsafeCell::new(None),
        previous_task: UnsafeCell::new(None),
    }));
    let address = area as *mut PerCpu as u64;
    area.self_ptr = address;
    // SAFETY: IA32_GS_BASE exists on every x86-64 processor. Nothing in the
    // kernel uses GS for anything else, and the area is leaked, so it lives
    // as long as the pointer is in use.
    unsafe { msr::write(msr::IA32_GS_BASE, address) };
    ONLINE.fetch_add(1, Ordering::Relaxed);
    READY.store(true, Ordering::Release);
}

/// The calling processor's area. Panics before `frame::init`.
pub fn current() -> &'static PerCpu {
    assert!(
        READY.load(Ordering::Acquire),
        "cpu: current() before frame::init()"
    );
    let address: u64;
    // SAFETY: GS_BASE points at this processor's leaked PerCpu, whose first
    // field holds its own address; reading it has no side effects.
    unsafe {
        asm!("mov {}, gs:[0]", out(reg) address, options(nostack, preserves_flags, readonly))
    };
    // SAFETY: `address` is the PerCpu set up by `init_this_cpu`, leaked and
    // never mutated except through atomics and the task cells, which only
    // this processor touches with interrupts disabled.
    unsafe { &*(address as *const PerCpu) }
}

/// Number of processors running STELA.
pub fn online() -> usize {
    ONLINE.load(Ordering::Relaxed)
}
