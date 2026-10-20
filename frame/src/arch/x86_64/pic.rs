// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The legacy 8259A interrupt controllers, which STELA turns off in favour
//! of the APIC (OSDev Wiki "8259 PIC"; Intel 8259A datasheet, ICW1-ICW4 and
//! OCW1).
//!
//! Before masking every input, the pair is remapped to vectors 32-47: the
//! firmware's mapping may overlap the CPU exception vectors (the PC/AT
//! default puts IRQ 0-7 at vectors 8-15), and even a masked 8259 can
//! signal a spurious IRQ 7 or 15, which must not look like an exception.

use super::io::{inb, outb};

const MASTER_COMMAND: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_COMMAND: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;

/// ICW1: initialisation, ICW4 follows (bit 0), cascade mode, edge triggered.
const ICW1_INIT: u8 = 0x11;
/// ICW4: 8086 mode.
const ICW4_8086: u8 = 0x01;

/// A write to an unused port (0x80, the POST code port) takes about 1 µs
/// and gives an old 8259 time to settle between initialisation words.
fn io_wait() {
    // SAFETY: port 0x80 is the POST diagnostic port; writing it has no
    // effect on memory or other devices.
    unsafe { outb(0x80, 0) };
}

/// Remaps the master to vectors `base..base + 8` and the slave to
/// `base + 8..base + 16`, then masks all 16 inputs.
pub(crate) fn disable(base: u8) {
    // SAFETY: the 8259 ports belong to this module; reprogramming and
    // masking the controllers only stops them from raising interrupts.
    unsafe {
        outb(MASTER_COMMAND, ICW1_INIT);
        io_wait();
        outb(SLAVE_COMMAND, ICW1_INIT);
        io_wait();
        outb(MASTER_DATA, base); // ICW2: vector offset
        io_wait();
        outb(SLAVE_DATA, base + 8);
        io_wait();
        outb(MASTER_DATA, 1 << 2); // ICW3: slave on IRQ 2
        io_wait();
        outb(SLAVE_DATA, 2); // ICW3: slave identity 2
        io_wait();
        outb(MASTER_DATA, ICW4_8086);
        io_wait();
        outb(SLAVE_DATA, ICW4_8086);
        io_wait();
        outb(MASTER_DATA, 0xFF); // OCW1: mask everything
        outb(SLAVE_DATA, 0xFF);
    }
}

/// The two interrupt mask registers (master in the low byte); 0xFFFF when
/// everything is masked.
pub(crate) fn masks() -> u16 {
    // SAFETY: reading OCW1 back from the data ports has no side effects.
    unsafe { u16::from(inb(MASTER_DATA)) | (u16::from(inb(SLAVE_DATA)) << 8) }
}
