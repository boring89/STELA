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
//! each one calls [`init_this_cpu`] for itself. User mode (P3) will need
//! `swapgs` and IA32_KERNEL_GS_BASE.

use crate::arch::x86_64::msr;
use alloc::boxed::Box;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// The data of one processor. `self_ptr` must stay the first field.
#[repr(C)]
#[derive(Debug)]
pub struct PerCpu {
    /// The address of this structure (read through `gs:[0]`).
    self_ptr: u64,
    id: u32,
    apic_id: u32,
    ticks: AtomicU64,
}

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
    // never mutated except through atomics.
    unsafe { &*(address as *const PerCpu) }
}

/// Number of processors running STELA.
pub fn online() -> usize {
    ONLINE.load(Ordering::Relaxed)
}
