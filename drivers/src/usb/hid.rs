// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! HID boot protocol reports (HID 1.11 appendix B) turned into STELA's
//! input events (`abi::key`), the same ones the PS/2 drivers make.
//!
//! ```text
//!  keyboard (8 bytes): modifiers, reserved, up to 6 key usages held
//!     modifiers: bit 0 LCtrl 1 LShift 2 LAlt 3 LGUI 4 RCtrl 5 RShift 6 RAlt 7 RGUI
//!  mouse (3+ bytes):   buttons (bit 0 left, 1 right, 2 middle), dx, dy (i8),
//!                      [wheel (i8)]
//! ```
//!
//! A keyboard report is the whole state, so events come from comparing it
//! with the previous one: releases first, then modifier changes, then
//! presses (so Ctrl+C arrives as Ctrl down, then C). Usages are from the
//! HID Usage Tables 1.12, chapter 10 (Keyboard/Keypad Page 0x07).

use abi::key::{KeyCode, KeyEvent, Modifiers, MouseEvent};
use alloc::vec::Vec;

/// The key for a Keyboard/Keypad page usage (`None` for the ones STELA
/// does not name; those come through as `KeyCode::Unknown(0x200 | usage)`).
pub fn usage_to_key(usage: u8) -> Option<KeyCode> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    ];
    const DIGITS: [KeyCode; 10] = [
        Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0,
    ];
    const FUNCTION: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    Some(match usage {
        0x04..=0x1D => LETTERS[usize::from(usage - 0x04)],
        0x1E..=0x27 => DIGITS[usize::from(usage - 0x1E)],
        0x28 => Enter,
        0x29 => Escape,
        0x2A => Backspace,
        0x2B => Tab,
        0x2C => Space,
        0x2D => Minus,
        0x2E => Equal,
        0x2F => LeftBracket,
        0x30 => RightBracket,
        0x31 => Backslash,
        0x33 => Semicolon,
        0x34 => Quote,
        0x35 => Backquote,
        0x36 => Comma,
        0x37 => Period,
        0x38 => Slash,
        0x39 => CapsLock,
        0x3A..=0x45 => FUNCTION[usize::from(usage - 0x3A)],
        0x46 => PrintScreen,
        0x47 => ScrollLock,
        0x48 => Pause,
        0x49 => Insert,
        0x4A => Home,
        0x4B => PageUp,
        0x4C => Delete,
        0x4D => End,
        0x4E => PageDown,
        0x4F => Right,
        0x50 => Left,
        0x51 => Down,
        0x52 => Up,
        0x53 => NumLock,
        0x65 => Menu,
        _ => return None,
    })
}

/// The modifier keys in report bit order.
const MODIFIER_KEYS: [KeyCode; 8] = [
    KeyCode::LeftCtrl,
    KeyCode::LeftShift,
    KeyCode::LeftAlt,
    KeyCode::LeftMeta,
    KeyCode::RightCtrl,
    KeyCode::RightShift,
    KeyCode::RightAlt,
    KeyCode::RightMeta,
];

/// The state of one boot keyboard.
#[derive(Debug, Default)]
pub struct Keyboard {
    modifiers: u8,
    keys: [u8; 6],
    caps_lock: bool,
}

impl Keyboard {
    /// A keyboard with nothing held.
    pub const fn new() -> Keyboard {
        Keyboard {
            modifiers: 0,
            keys: [0; 6],
            caps_lock: false,
        }
    }

    fn modifiers(&self) -> Modifiers {
        let m = self.modifiers;
        Modifiers {
            ctrl: m & 0x11 != 0,
            shift: m & 0x22 != 0,
            alt: m & 0x44 != 0,
            meta: m & 0x88 != 0,
            caps_lock: self.caps_lock,
        }
    }

    fn event(&self, code: KeyCode, pressed: bool) -> KeyEvent {
        KeyEvent {
            code,
            pressed,
            modifiers: self.modifiers(),
        }
    }

    /// The events a new report means.
    pub fn report(&mut self, report: &[u8]) -> Vec<KeyEvent> {
        let mut events = Vec::new();
        let Some(r) = report.get(..8) else {
            return events;
        };
        // All 1s (ErrorRollOver) means too many keys: no change.
        if r[2..8].iter().all(|&k| k == 1) {
            return events;
        }
        let mut new = [0u8; 6];
        new.copy_from_slice(&r[2..8]);
        let code = |u: u8| usage_to_key(u).unwrap_or(KeyCode::Unknown(0x200 | u16::from(u)));
        for &old in self.keys.iter().filter(|&&k| k > 3) {
            if !new.contains(&old) {
                events.push(self.event(code(old), false));
            }
        }
        for (bit, &key) in MODIFIER_KEYS.iter().enumerate() {
            let mask = 1u8 << bit;
            let (was, now) = (self.modifiers & mask != 0, r[0] & mask != 0);
            if was != now {
                self.modifiers ^= mask;
                events.push(self.event(key, now));
            }
        }
        for &key in new.iter().filter(|&&k| k > 3) {
            if !self.keys.contains(&key) {
                if usage_to_key(key) == Some(KeyCode::CapsLock) {
                    self.caps_lock = !self.caps_lock;
                }
                events.push(self.event(code(key), true));
            }
        }
        self.keys = new;
        events
    }
}

/// A boot mouse report as a mouse event (USB's dy is already down =
/// positive, like the screen).
pub fn mouse_report(report: &[u8]) -> Option<MouseEvent> {
    let r = report.get(..3)?;
    Some(MouseEvent {
        dx: i16::from(r[1] as i8),
        dy: i16::from(r[2] as i8),
        buttons: r[0] & 7,
        wheel: report.get(3).map_or(0, |&w| w as i8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::key::key_to_char;

    #[test]
    fn usages() {
        assert_eq!(usage_to_key(0x04), Some(KeyCode::A));
        assert_eq!(usage_to_key(0x1D), Some(KeyCode::Z));
        assert_eq!(usage_to_key(0x1E), Some(KeyCode::Digit1));
        assert_eq!(usage_to_key(0x27), Some(KeyCode::Digit0));
        assert_eq!(usage_to_key(0x28), Some(KeyCode::Enter));
        assert_eq!(usage_to_key(0x45), Some(KeyCode::F12));
        assert_eq!(usage_to_key(0x52), Some(KeyCode::Up));
        assert_eq!(usage_to_key(0x59), None); // keypad 1
    }

    #[test]
    fn typing() {
        let mut k = Keyboard::new();
        // "s" down, up.
        let e = k.report(&[0, 0, 0x16, 0, 0, 0, 0, 0]);
        assert_eq!(e.len(), 1);
        assert_eq!(key_to_char(&e[0]), Some('s'));
        let e = k.report(&[0; 8]);
        assert_eq!((e[0].code, e[0].pressed), (KeyCode::S, false));
        // Shift + "1" in one report: shift first.
        let e = k.report(&[0x02, 0, 0x1E, 0, 0, 0, 0, 0]);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].code, e[0].pressed), (KeyCode::LeftShift, true));
        assert_eq!(key_to_char(&e[1]), Some('!'));
        // Ctrl+C: C with ctrl held.
        k.report(&[0; 8]);
        let e = k.report(&[0x10, 0, 0x06, 0, 0, 0, 0, 0]);
        assert_eq!(key_to_char(&e[1]), Some('\u{3}'));
        // Release in one report: key up before the modifier.
        let e = k.report(&[0; 8]);
        assert_eq!((e[0].code, e[1].code), (KeyCode::C, KeyCode::RightCtrl));
        // Rollover errors change nothing.
        assert!(k.report(&[0, 0, 1, 1, 1, 1, 1, 1]).is_empty());
        // Caps Lock toggles.
        k.report(&[0, 0, 0x39, 0, 0, 0, 0, 0]);
        k.report(&[0; 8]);
        let e = k.report(&[0, 0, 0x04, 0, 0, 0, 0, 0]);
        assert_eq!(key_to_char(&e[0]), Some('A'));
        // Unnamed keys still come through.
        k.report(&[0; 8]);
        let e = k.report(&[0, 0, 0x59, 0, 0, 0, 0, 0]);
        assert_eq!(e[0].code, KeyCode::Unknown(0x259));
    }

    #[test]
    fn mice() {
        let m = mouse_report(&[1, 0xFB, 3]).unwrap();
        assert_eq!((m.dx, m.dy, m.buttons, m.wheel), (-5, 3, 1, 0));
        let m = mouse_report(&[0, 0, 0, 0xFF]).unwrap();
        assert_eq!(m.wheel, -1);
        assert_eq!(mouse_report(&[1, 2]), None);
    }
}
