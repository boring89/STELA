// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The first serial port (COM1), a 16550-compatible UART at I/O port `0x3F8`.
//!
//! Register map and the initialisation sequence follow OSDev Wiki "Serial
//! Ports". STELA only transmits, by polling; no interrupts are used.

use crate::arch::x86_64::io::{inb, outb};
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

/// I/O base of COM1.
const COM1: u16 = 0x3F8;

// Register offsets from the base port (OSDev Wiki "Serial Ports").
/// Transmit holding register (write), or divisor low byte when DLAB = 1.
const DATA: u16 = 0;
/// Interrupt enable register, or divisor high byte when DLAB = 1.
const INT_ENABLE: u16 = 1;
/// FIFO control register (write).
const FIFO_CTRL: u16 = 2;
/// Line control register: data bits, parity, stop bits, DLAB (bit 7).
const LINE_CTRL: u16 = 3;
/// Modem control register: DTR (bit 0), RTS (bit 1).
const MODEM_CTRL: u16 = 4;
/// Line status register: bit 5 = transmit holding register empty.
const LINE_STATUS: u16 = 5;

/// Line status bit 5: the UART can accept another byte.
const LSR_THR_EMPTY: u8 = 1 << 5;

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Programs COM1 for 38400 baud, 8 data bits, no parity, one stop bit, with
/// FIFOs on and interrupts off. Calling it again does nothing.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: COM1 belongs to this module; these writes only configure the
    // UART (OSDev Wiki "Serial Ports", "Initialization").
    unsafe {
        outb(COM1 + INT_ENABLE, 0x00); // no interrupts
        outb(COM1 + LINE_CTRL, 0x80); // DLAB = 1: next two writes set the divisor
        outb(COM1 + DATA, 0x03); // divisor 3 = 115200 / 3 = 38400 baud
        outb(COM1 + INT_ENABLE, 0x00); // divisor high byte
        outb(COM1 + LINE_CTRL, 0x03); // DLAB = 0, 8 bits, no parity, 1 stop bit
        outb(COM1 + FIFO_CTRL, 0xC7); // FIFOs on, cleared, 14-byte threshold
        outb(COM1 + MODEM_CTRL, 0x03); // DTR + RTS
    }
}

/// Sends one byte, waiting until the UART can take it.
pub fn write_byte(byte: u8) {
    if !INITIALIZED.load(Ordering::Acquire) {
        init();
    }
    // SAFETY: reading the line status register of COM1 has no side effects
    // beyond reporting status.
    while unsafe { inb(COM1 + LINE_STATUS) } & LSR_THR_EMPTY == 0 {
        core::hint::spin_loop();
    }
    // SAFETY: the transmit register is empty; writing it sends the byte.
    unsafe { outb(COM1 + DATA, byte) };
}

/// Sends a string, turning `\n` into `\r\n` for terminals.
pub fn write_str(s: &str) {
    for byte in s.bytes() {
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
}

/// A handle for formatted output to COM1 (`write!(Serial, ...)`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Serial;

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}
