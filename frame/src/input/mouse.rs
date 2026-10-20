// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding PS/2 mouse packets (OSDev Wiki "PS/2 Mouse"; the standard
//! 3-byte protocol, no wheel). Pure logic, tested on the host.
//!
//! ```text
//!  byte 0  bit 7 Y overflow  bit 6 X overflow  bit 5 Y sign  bit 4 X sign
//!          bit 3 always 1    bit 2 middle      bit 1 right   bit 0 left
//!  byte 1  X motion (low 8 bits of a 9-bit two's complement value)
//!  byte 2  Y motion (up is positive)
//! ```
//!
//! A byte 0 without bit 3 means the stream is out of step (a byte was
//! lost); the decoder drops bytes until it finds one with bit 3 set.

use abi::key::MouseEvent;

const ALWAYS_ONE: u8 = 1 << 3;
const X_SIGN: u8 = 1 << 4;
const Y_SIGN: u8 = 1 << 5;
const X_OVERFLOW: u8 = 1 << 6;
const Y_OVERFLOW: u8 = 1 << 7;

/// Collects the bytes of one packet.
#[derive(Debug, Default)]
pub struct MouseDecoder {
    bytes: [u8; 3],
    have: usize,
}

impl MouseDecoder {
    /// A decoder at the start of a packet.
    pub const fn new() -> MouseDecoder {
        MouseDecoder {
            bytes: [0; 3],
            have: 0,
        }
    }

    /// Takes one byte; returns the event when a packet is complete.
    pub fn feed(&mut self, byte: u8) -> Option<MouseEvent> {
        if self.have == 0 && byte & ALWAYS_ONE == 0 {
            return None;
        }
        self.bytes[self.have] = byte;
        self.have += 1;
        if self.have < 3 {
            return None;
        }
        self.have = 0;
        let [flags, x, y] = self.bytes;
        let mut dx = i16::from(x) - if flags & X_SIGN != 0 { 256 } else { 0 };
        let mut dy = i16::from(y) - if flags & Y_SIGN != 0 { 256 } else { 0 };
        // An overflowed axis carries no usable value.
        if flags & X_OVERFLOW != 0 {
            dx = 0;
        }
        if flags & Y_OVERFLOW != 0 {
            dy = 0;
        }
        Some(MouseEvent {
            dx,
            dy: -dy,
            buttons: flags & 0x07,
            wheel: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::key::button;

    #[test]
    fn packets() {
        let mut d = MouseDecoder::new();
        assert_eq!(d.feed(0x08), None);
        assert_eq!(d.feed(10), None);
        let e = d.feed(5).unwrap();
        assert_eq!((e.dx, e.dy, e.buttons), (10, -5, 0));
        // Left button, moving left and down (negative X, negative Y).
        d.feed(0x08 | X_SIGN | Y_SIGN | 1);
        d.feed(0xF6);
        let e = d.feed(0xFE).unwrap();
        assert_eq!((e.dx, e.dy, e.buttons), (-10, 2, button::LEFT));
    }

    #[test]
    fn resynchronises_and_ignores_overflow() {
        let mut d = MouseDecoder::new();
        // A stray byte without bit 3 is dropped.
        assert_eq!(d.feed(0x05), None);
        d.feed(0x08 | X_OVERFLOW | 2);
        d.feed(0xFF);
        let e = d.feed(3).unwrap();
        assert_eq!((e.dx, e.dy, e.buttons), (0, -3, button::RIGHT));
    }
}
