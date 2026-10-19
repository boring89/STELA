// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Port I/O: the `in` and `out` instructions (Intel SDM Vol. 2A "IN", Vol. 2B
//! "OUT"; Vol. 1 Ch. 19 "Input/Output").
//!
//! Writing to an arbitrary port can reprogram any device, so these stay
//! `unsafe` and crate-private; safe drivers in this crate wrap the ports they
//! own.

use core::arch::asm;

/// Writes one byte to an I/O port.
///
/// # Safety
///
/// The caller must own `port` and know that writing `value` to it has no
/// effect on memory safety (for example it must not start a DMA transfer
/// into memory Rust believes it owns).
pub(crate) unsafe fn outb(port: u16, value: u8) {
    // SAFETY: the caller guarantees the write is harmless; `out` does not
    // touch memory or the stack.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags))
    };
}

/// Reads one byte from an I/O port.
///
/// # Safety
///
/// The caller must own `port`; reads can have side effects on the device
/// (for example acknowledging a status bit).
pub(crate) unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: the caller guarantees the read is harmless; `in` does not touch
    // memory or the stack.
    unsafe {
        asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags))
    };
    value
}

/// Writes one 32-bit value to an I/O port.
///
/// # Safety
///
/// Same as [`outb`].
pub(crate) unsafe fn outl(port: u16, value: u32) {
    // SAFETY: the caller guarantees the write is harmless; `out` does not
    // touch memory or the stack.
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags))
    };
}
