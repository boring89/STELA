// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Interrupts: the vector layout, handler registration and dispatch.
//!
//! ```text
//! vector    use
//! 0-31      CPU exceptions (arch/x86_64/exceptions.rs)
//! 32-47     legacy ISA IRQ 0-15, through the I/O APIC (IRQ n -> 32 + n)
//! 48        local APIC timer
//! 49        reserved for IPIs
//! 50-254    free for drivers
//! 255       local APIC spurious vector (no EOI)
//! ```
//!
//! Every vector from 32 up enters through the same stub as the exceptions,
//! so a handler sees the interrupted registers as a [`TrapFrame`]. Handlers
//! are plain `fn` pointers, run with interrupts disabled and must be short;
//! after the handler returns, [`dispatch`] signals the end of the interrupt
//! (EOI) to the local APIC. A vector without a handler is reported once and
//! acknowledged, not treated as fatal.

use crate::arch::x86_64::{apic, interrupts, ioapic, pic};
use crate::sync::SpinLock;
use crate::trap::TrapFrame;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub use crate::acpi::tables::{Polarity, Trigger};

/// First interrupt vector after the exceptions.
pub const IRQ_BASE: u8 = 32;
/// Vector of the local APIC timer.
pub const TIMER_VECTOR: u8 = 48;
/// Vector reserved for inter-processor interrupts.
pub const IPI_VECTOR: u8 = 49;
/// First vector free for drivers.
pub const FIRST_FREE_VECTOR: u8 = 50;
/// The local APIC's spurious-interrupt vector.
pub const SPURIOUS_VECTOR: u8 = 0xFF;

/// The vector of legacy ISA IRQ `irq` (0-15).
pub const fn isa_vector(irq: u8) -> u8 {
    IRQ_BASE + irq
}

/// An interrupt handler. It runs with interrupts disabled; the EOI is sent
/// after it returns.
pub type Handler = fn(&TrapFrame);

/// Why [`register`] refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqError {
    /// The vector is an exception or the spurious vector.
    Reserved,
    /// The vector already has a handler.
    Busy,
    /// No I/O APIC input for that IRQ.
    NoRoute,
}

static HANDLERS: SpinLock<[Option<Handler>; 256]> = SpinLock::new([None; 256]);
static COUNTS: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
static WARNED: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];

/// Installs `handler` for `vector` (32-254).
pub fn register(vector: u8, handler: Handler) -> Result<(), IrqError> {
    if vector < IRQ_BASE || vector == SPURIOUS_VECTOR {
        return Err(IrqError::Reserved);
    }
    let mut handlers = HANDLERS.lock();
    let slot = &mut handlers[usize::from(vector)];
    if slot.is_some() {
        return Err(IrqError::Busy);
    }
    *slot = Some(handler);
    Ok(())
}

/// Removes the handler of `vector`, if any.
pub fn unregister(vector: u8) {
    HANDLERS.lock()[usize::from(vector)] = None;
}

/// How many times `vector` has arrived.
pub fn count(vector: u8) -> u64 {
    COUNTS[usize::from(vector)].load(Ordering::Relaxed)
}

/// Called by the common stub for vectors 32-255.
pub(crate) fn dispatch(frame: &TrapFrame) {
    let vector = (frame.vector & 0xFF) as u8;
    COUNTS[usize::from(vector)].fetch_add(1, Ordering::Relaxed);
    if vector == SPURIOUS_VECTOR {
        // Spurious interrupts are not in service: no EOI (Intel SDM Vol. 3A
        // "Spurious Interrupt").
        return;
    }
    // Copy the handler out so the lock is not held while it runs.
    let handler = HANDLERS.lock()[usize::from(vector)];
    match handler {
        Some(handler) => handler(frame),
        None => {
            if !WARNED[usize::from(vector)].swap(true, Ordering::Relaxed) {
                crate::kprintln!("irq: vector {vector} has no handler (reported once)");
            }
        }
    }
    apic::eoi();
}

/// Masks the 8259s, enables the local APIC of the boot processor, sets up
/// its per-CPU area and masks every I/O APIC input. Interrupts stay
/// disabled; `frame::init` enables them at the end.
pub(crate) fn init(madt: &crate::acpi::tables::Madt) {
    pic::disable(IRQ_BASE);
    let apic_id = apic::init(madt, SPURIOUS_VECTOR);
    crate::cpu::init_this_cpu(0, apic_id);
    ioapic::init(madt);
}

/// Sends `vector` to this processor through its local APIC (a self-IPI).
/// With interrupts enabled it arrives right away; otherwise it stays
/// pending until they are enabled.
pub fn self_ipi(vector: u8) {
    apic::self_ipi(vector);
}

/// Routes legacy ISA IRQ `irq` to vector `32 + irq` on this processor,
/// using the MADT's interrupt source overrides for the GSI, polarity and
/// trigger mode, and unmasks it.
pub fn enable_isa(irq: u8) -> Result<(), IrqError> {
    let (gsi, polarity, trigger) = crate::acpi::madt().isa_irq(irq);
    let apic_id = crate::cpu::current().apic_id();
    if ioapic::route(gsi, isa_vector(irq), apic_id, polarity, trigger) {
        Ok(())
    } else {
        Err(IrqError::NoRoute)
    }
}

/// Masks legacy ISA IRQ `irq` at the I/O APIC.
pub fn disable_isa(irq: u8) {
    let (gsi, _, _) = crate::acpi::madt().isa_irq(irq);
    ioapic::mask(gsi);
}

/// The I/O APIC redirection entry (low half) of ISA IRQ `irq`, for tests.
pub fn isa_entry(irq: u8) -> Option<u32> {
    ioapic::entry(crate::acpi::madt().isa_irq(irq).0)
}

/// The 8259 mask registers (0xFFFF = all masked), for tests.
pub fn legacy_pic_masks() -> u16 {
    pic::masks()
}

/// Enables interrupts.
pub(crate) fn enable() {
    interrupts::enable();
}
