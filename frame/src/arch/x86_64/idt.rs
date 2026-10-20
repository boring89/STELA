// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The interrupt descriptor table (IDT): 256 gates, one per vector
//! (Intel SDM Vol. 3A 6.10 "Interrupt Descriptor Table", 6.14.1 "64-Bit Mode
//! IDT", Figure 6-8 "64-Bit IDT Gate Descriptors").
//!
//! Every vector has a gate: 0-31 lead to the exception handlers, 32-255 to
//! the interrupt stubs, which hand the vector to `crate::irq`. All are
//! interrupt gates, so IF is clear while a handler runs.

use super::gdt::{DOUBLE_FAULT_IST, DescriptorTablePointer, KERNEL_CODE};
use crate::sync::RacyCell;
use core::arch::asm;
use core::mem::size_of;

/// One 16-byte gate.
#[derive(Clone, Copy)]
#[repr(C)]
struct Gate {
    offset_low: u16,
    selector: u16,
    /// Bits 0-2: IST index (0 = do not switch stacks).
    ist: u8,
    /// P (bit 7), DPL (bits 5-6), type (bits 0-3).
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

/// Present, DPL 0, type 0xE: 64-bit interrupt gate (clears IF on entry).
const INTERRUPT_GATE: u8 = 0x8E;

impl Gate {
    const MISSING: Gate = Gate {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    const fn new(handler: u64, ist: u8) -> Gate {
        Gate {
            offset_low: handler as u16,
            selector: KERNEL_CODE,
            ist,
            attributes: INTERRUPT_GATE,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

const _: () = assert!(size_of::<Gate>() == 16);

#[repr(C, align(16))]
struct Idt([Gate; 256]);

static IDT: RacyCell<Idt> = RacyCell::new(Idt([Gate::MISSING; 256]));

/// Fills the 256 gates from `stubs` and loads the IDT.
///
/// # Safety
///
/// Must be called once, on the boot processor, after the GDT is loaded and
/// before interrupts are enabled. `stubs` must be the entry points of
/// handlers that follow the interrupt-gate calling convention.
pub(crate) unsafe fn init(stubs: &[u64; 256]) {
    let idt = IDT.get();
    // SAFETY: single-threaded first use (caller's guarantee); no other
    // reference to IDT exists.
    unsafe {
        for (vector, &stub) in stubs.iter().enumerate() {
            let ist = if vector == 8 { DOUBLE_FAULT_IST } else { 0 };
            (*idt).0[vector] = Gate::new(stub, ist);
        }
    }
    let pointer = DescriptorTablePointer {
        limit: (size_of::<Idt>() - 1) as u16,
        base: idt as u64,
    };
    // SAFETY: the IDT is a static that lives forever and its gates point at
    // valid handlers (caller's guarantee).
    unsafe { asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags)) };
}

/// Loads the (already filled) IDT on an application processor.
pub(crate) fn load() {
    let pointer = DescriptorTablePointer {
        limit: (size_of::<Idt>() - 1) as u16,
        base: IDT.get() as u64,
    };
    // SAFETY: the IDT was filled by the boot processor's `init` and is
    // never changed afterwards; it lives forever.
    unsafe { asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags)) };
}
