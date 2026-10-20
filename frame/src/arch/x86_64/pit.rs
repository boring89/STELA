// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The 8254 programmable interval timer (PIT), used once at boot as a
//! reference clock: its input runs at a fixed 1.193182 MHz on every PC,
//! so counting how far the TSC and the APIC timer advance during a PIT
//! interval gives their frequencies (OSDev Wiki "Programmable Interval
//! Timer", "APIC timer"; Intel 8254 datasheet, mode 0).
//!
//! Channel 2 is used because its gate is software controlled (port 0x61
//! bit 0) and its output can be read back (port 0x61 bit 5), so no
//! interrupt is needed. Port 0x61 bit 1 would connect the output to the PC
//! speaker; it is kept clear so the calibration is silent.

use super::io::{inb, outb};

/// The PIT's input clock in Hz.
pub(crate) const PIT_HZ: u64 = 1_193_182;

const CHANNEL2_DATA: u16 = 0x42;
const COMMAND: u16 = 0x43;
/// Port B of the old 8255: bit 0 gate of channel 2, bit 1 speaker data
/// enable, bit 5 output of channel 2.
const PORT_B: u16 = 0x61;
const GATE2: u8 = 1 << 0;
const SPEAKER: u8 = 1 << 1;
const OUT2: u8 = 1 << 5;

/// Command: channel 2 (bits 6-7 = 10), low then high byte (bits 4-5 = 11),
/// mode 0 "interrupt on terminal count" (bits 1-3 = 000), binary.
const CHANNEL2_MODE0: u8 = 0b1011_0000;

/// Polls of port B before deciding the PIT is not counting (some recent
/// platforms gate the 8254 off). A port read takes about a microsecond on
/// real hardware, so this is seconds, far beyond any window used here.
const MAX_POLLS: u32 = 10_000_000;

/// Counts `count` PIT clocks on channel 2 and returns `sample()` taken just
/// before the countdown starts and just after it ends, or `None` if the
/// countdown never ends. Interrupts should be off so that nothing stretches
/// the window between the samples.
pub(crate) fn measure<T>(count: u16, mut sample: impl FnMut() -> T) -> Option<(T, T)> {
    // SAFETY: ports 0x42, 0x43 and 0x61 belong to this module (channel 2
    // and its gate are not used by anything else); the writes only program
    // the timer and keep the speaker off.
    let start = unsafe {
        let port_b = inb(PORT_B);
        outb(PORT_B, (port_b & !SPEAKER) | GATE2);
        outb(COMMAND, CHANNEL2_MODE0);
        outb(CHANNEL2_DATA, count as u8);
        // In mode 0, loading the high byte starts the countdown (with the
        // gate high) and drives the output low until it reaches zero.
        let start = sample();
        outb(CHANNEL2_DATA, (count >> 8) as u8);
        start
    };
    let mut polls = 0;
    // SAFETY: reading port B has no side effects.
    while unsafe { inb(PORT_B) } & OUT2 == 0 && polls < MAX_POLLS {
        polls += 1;
        core::hint::spin_loop();
    }
    let end = sample();
    // SAFETY: as above; this closes the gate again.
    unsafe {
        let port_b = inb(PORT_B);
        outb(PORT_B, port_b & !(SPEAKER | GATE2));
    }
    (polls < MAX_POLLS).then_some((start, end))
}
