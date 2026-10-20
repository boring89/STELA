// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! STELA keys to Apple Desktop Bus key codes (the codes an ADB keyboard
//! sends; "Inside Macintosh: Text", Figure 2-10, the same table as
//! 68k_HAL's `kernel/mac/adbkeys.c`).
//!
//! Modifiers: Ctrl is Control, Alt is Command (Alt-N is Command-N, as in
//! most Mac emulators), the Windows/Meta keys and AltGr are Option. ADB
//! reports right-hand modifiers with the left-hand codes.

use abi::key::KeyCode;

/// The ADB key code of `key`, `None` for keys the Mac keyboard lacks.
pub fn adb_code(key: KeyCode) -> Option<u8> {
    use KeyCode::*;
    Some(match key {
        A => 0x00,
        S => 0x01,
        D => 0x02,
        F => 0x03,
        H => 0x04,
        G => 0x05,
        Z => 0x06,
        X => 0x07,
        C => 0x08,
        V => 0x09,
        B => 0x0B,
        Q => 0x0C,
        W => 0x0D,
        E => 0x0E,
        R => 0x0F,
        Y => 0x10,
        T => 0x11,
        Digit1 => 0x12,
        Digit2 => 0x13,
        Digit3 => 0x14,
        Digit4 => 0x15,
        Digit6 => 0x16,
        Digit5 => 0x17,
        Equal => 0x18,
        Digit9 => 0x19,
        Digit7 => 0x1A,
        Minus => 0x1B,
        Digit8 => 0x1C,
        Digit0 => 0x1D,
        RightBracket => 0x1E,
        O => 0x1F,
        U => 0x20,
        LeftBracket => 0x21,
        I => 0x22,
        P => 0x23,
        Enter => 0x24,
        L => 0x25,
        J => 0x26,
        Quote => 0x27,
        K => 0x28,
        Semicolon => 0x29,
        Backslash => 0x2A,
        Comma => 0x2B,
        Slash => 0x2C,
        N => 0x2D,
        M => 0x2E,
        Period => 0x2F,
        Tab => 0x30,
        Space => 0x31,
        Backquote => 0x32,
        // The Mac's "delete" erases backwards.
        Backspace => 0x33,
        Escape => 0x35,
        LeftCtrl | RightCtrl => 0x36,
        LeftAlt => 0x37,
        LeftShift | RightShift => 0x38,
        CapsLock => 0x39,
        RightAlt | LeftMeta | RightMeta => 0x3A,
        F5 => 0x60,
        F6 => 0x61,
        F7 => 0x62,
        F3 => 0x63,
        F8 => 0x64,
        F9 => 0x65,
        F11 => 0x67,
        F10 => 0x6D,
        F12 => 0x6F,
        // The Help key.
        Insert => 0x72,
        Home => 0x73,
        PageUp => 0x74,
        // Forward delete.
        Delete => 0x75,
        F4 => 0x76,
        End => 0x77,
        F2 => 0x78,
        PageDown => 0x79,
        F1 => 0x7A,
        Left => 0x7B,
        Right => 0x7C,
        Down => 0x7D,
        Up => 0x7E,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_modifiers() {
        assert_eq!(adb_code(KeyCode::A), Some(0x00));
        assert_eq!(adb_code(KeyCode::Z), Some(0x06));
        assert_eq!(adb_code(KeyCode::Digit0), Some(0x1D));
        assert_eq!(adb_code(KeyCode::Enter), Some(0x24));
        assert_eq!(adb_code(KeyCode::LeftAlt), Some(0x37));
        assert_eq!(adb_code(KeyCode::RightShift), adb_code(KeyCode::LeftShift));
        assert_eq!(adb_code(KeyCode::Up), Some(0x7E));
        assert_eq!(adb_code(KeyCode::PrintScreen), None);
        assert_eq!(adb_code(KeyCode::Unknown(0x55)), None);
    }

    #[test]
    fn codes_are_unique_and_in_range() {
        let mut seen = [None; 128];
        for key in KeyCode::NAMED {
            let Some(code) = adb_code(key) else { continue };
            assert!(code < 0x80, "{key:?}");
            // Left and right modifiers share codes; nothing else does.
            if let Some(other) = seen[code as usize] {
                let shared = [0x36, 0x38, 0x3A];
                assert!(shared.contains(&code), "{key:?} and {other:?}");
            }
            seen[code as usize] = Some(key);
        }
    }
}
