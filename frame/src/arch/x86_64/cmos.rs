// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The CMOS real-time clock's registers (OSDev Wiki "CMOS", "RTC"):
//! port 0x70 selects a register, port 0x71 reads it.
//!
//! The RTC updates its registers once a second; while status register A
//! bit 7 ("update in progress") is set the values may be half-written. So
//! the registers are read after the flag clears, then read again until two
//! reads in a row agree.
//!
//! Bit 7 of the value written to port 0x70 disables NMIs; it is left clear
//! (NMIs stay as they are).

use super::io::{inb, outb};
use crate::time::rtc::Registers;

const INDEX: u16 = 0x70;
const DATA: u16 = 0x71;

const SECONDS: u8 = 0x00;
const MINUTES: u8 = 0x02;
const HOURS: u8 = 0x04;
const DAY: u8 = 0x07;
const MONTH: u8 = 0x08;
const YEAR: u8 = 0x09;
const STATUS_A: u8 = 0x0A;
const STATUS_B: u8 = 0x0B;
/// Status register A bit 7: an update is in progress.
const UPDATE_IN_PROGRESS: u8 = 1 << 7;

/// Polls of the update flag before giving up (an update takes under 2 ms;
/// a port read takes about a microsecond).
const MAX_POLLS: u32 = 100_000;
/// Attempts at two equal reads.
const MAX_READS: u32 = 10;

/// Reads CMOS register `index` (0-0x7F).
fn read(index: u8) -> u8 {
    // SAFETY: ports 0x70/0x71 belong to this module; selecting a register
    // and reading it only reads the CMOS (the NMI-disable bit 7 is clear).
    unsafe {
        outb(INDEX, index & 0x7F);
        inb(DATA)
    }
}

fn read_all(century: u8) -> Registers {
    Registers {
        second: read(SECONDS),
        minute: read(MINUTES),
        hour: read(HOURS),
        day: read(DAY),
        month: read(MONTH),
        year: read(YEAR),
        century: if century == 0 || century > 0x7F {
            0
        } else {
            read(century)
        },
        status_b: read(STATUS_B),
    }
}

fn wait_for_update() -> bool {
    for _ in 0..MAX_POLLS {
        if read(STATUS_A) & UPDATE_IN_PROGRESS == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// The RTC's registers, read consistently; `century` is the FADT's century
/// register index (0 = none). `None` if the RTC never settles.
pub(crate) fn read_registers(century: u8) -> Option<Registers> {
    if !wait_for_update() {
        return None;
    }
    let mut last = read_all(century);
    for _ in 0..MAX_READS {
        if !wait_for_update() {
            return None;
        }
        let now = read_all(century);
        if now == last {
            return Some(now);
        }
        last = now;
    }
    None
}
