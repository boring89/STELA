// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard events as user programs see them: STELA's key names
//! ([`KeyCode`], US layout), the modifier state, the US character mapping
//! ([`key_to_char`]) and the 4-byte encoding that carries a [`KeyEvent`] in
//! an `INPUT_KEY` message. The kernel's PS/2 decoder (`frame::input`)
//! produces these.

/// A key, named after its position on a US keyboard (not the character it
/// produces).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum KeyCode {
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Backquote,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Digit0,
    Minus,
    Equal,
    Backspace,
    Tab,
    Q,
    W,
    E,
    R,
    T,
    Y,
    U,
    I,
    O,
    P,
    LeftBracket,
    RightBracket,
    Backslash,
    CapsLock,
    A,
    S,
    D,
    F,
    G,
    H,
    J,
    K,
    L,
    Semicolon,
    Quote,
    Enter,
    LeftShift,
    Z,
    X,
    C,
    V,
    B,
    N,
    M,
    Comma,
    Period,
    Slash,
    RightShift,
    LeftCtrl,
    LeftMeta,
    LeftAlt,
    Space,
    RightAlt,
    RightMeta,
    Menu,
    RightCtrl,
    PrintScreen,
    ScrollLock,
    Pause,
    Insert,
    Home,
    PageUp,
    Delete,
    End,
    PageDown,
    Up,
    Left,
    Down,
    Right,
    NumLock,
    /// A key STELA does not name yet (the numeric keypad, multimedia
    /// keys): the make code, with 0x100 added for `0xE0`-prefixed keys.
    Unknown(u16),
}

/// Which modifier keys are down (and whether Caps Lock is on).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Either Shift.
    pub shift: bool,
    /// Either Ctrl.
    pub ctrl: bool,
    /// Either Alt.
    pub alt: bool,
    /// Either Meta (the Windows key).
    pub meta: bool,
    /// Caps Lock is toggled on.
    pub caps_lock: bool,
}

/// A key was pressed or released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key.
    pub code: KeyCode,
    /// Pressed (true) or released (false).
    pub pressed: bool,
    /// Modifier state after this event.
    pub modifiers: Modifiers,
}

impl KeyCode {
    /// Every named key, in encoding order: the encoding of a named key is
    /// its index here (stable: new keys are only appended).
    pub const NAMED: [KeyCode; 88] = [
        KeyCode::Escape,
        KeyCode::F1,
        KeyCode::F2,
        KeyCode::F3,
        KeyCode::F4,
        KeyCode::F5,
        KeyCode::F6,
        KeyCode::F7,
        KeyCode::F8,
        KeyCode::F9,
        KeyCode::F10,
        KeyCode::F11,
        KeyCode::F12,
        KeyCode::Backquote,
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
        KeyCode::Digit0,
        KeyCode::Minus,
        KeyCode::Equal,
        KeyCode::Backspace,
        KeyCode::Tab,
        KeyCode::Q,
        KeyCode::W,
        KeyCode::E,
        KeyCode::R,
        KeyCode::T,
        KeyCode::Y,
        KeyCode::U,
        KeyCode::I,
        KeyCode::O,
        KeyCode::P,
        KeyCode::LeftBracket,
        KeyCode::RightBracket,
        KeyCode::Backslash,
        KeyCode::CapsLock,
        KeyCode::A,
        KeyCode::S,
        KeyCode::D,
        KeyCode::F,
        KeyCode::G,
        KeyCode::H,
        KeyCode::J,
        KeyCode::K,
        KeyCode::L,
        KeyCode::Semicolon,
        KeyCode::Quote,
        KeyCode::Enter,
        KeyCode::LeftShift,
        KeyCode::Z,
        KeyCode::X,
        KeyCode::C,
        KeyCode::V,
        KeyCode::B,
        KeyCode::N,
        KeyCode::M,
        KeyCode::Comma,
        KeyCode::Period,
        KeyCode::Slash,
        KeyCode::RightShift,
        KeyCode::LeftCtrl,
        KeyCode::LeftMeta,
        KeyCode::LeftAlt,
        KeyCode::Space,
        KeyCode::RightAlt,
        KeyCode::RightMeta,
        KeyCode::Menu,
        KeyCode::RightCtrl,
        KeyCode::PrintScreen,
        KeyCode::ScrollLock,
        KeyCode::Pause,
        KeyCode::Insert,
        KeyCode::Home,
        KeyCode::PageUp,
        KeyCode::Delete,
        KeyCode::End,
        KeyCode::PageDown,
        KeyCode::Up,
        KeyCode::Left,
        KeyCode::Down,
        KeyCode::Right,
        KeyCode::NumLock,
    ];

    /// The 16-bit encoding: the index in [`KeyCode::NAMED`], or `0x8000 |
    /// code` for [`KeyCode::Unknown`].
    pub fn to_raw(self) -> u16 {
        if let KeyCode::Unknown(code) = self {
            return 0x8000 | (code & 0x7FFF);
        }
        let mut i = 0;
        while i < KeyCode::NAMED.len() {
            if KeyCode::NAMED[i] == self {
                return i as u16;
            }
            i += 1;
        }
        0x8000
    }

    /// The key with encoding `raw`, if any.
    pub fn from_raw(raw: u16) -> Option<KeyCode> {
        if raw & 0x8000 != 0 {
            return Some(KeyCode::Unknown(raw & 0x7FFF));
        }
        KeyCode::NAMED.get(usize::from(raw)).copied()
    }
}

impl Modifiers {
    /// Bit 0 Shift, 1 Ctrl, 2 Alt, 3 Meta, 4 Caps Lock.
    pub const fn to_raw(self) -> u8 {
        (self.shift as u8)
            | (self.ctrl as u8) << 1
            | (self.alt as u8) << 2
            | (self.meta as u8) << 3
            | (self.caps_lock as u8) << 4
    }

    /// The inverse of [`Modifiers::to_raw`] (other bits ignored).
    pub const fn from_raw(raw: u8) -> Modifiers {
        Modifiers {
            shift: raw & 1 != 0,
            ctrl: raw & 2 != 0,
            alt: raw & 4 != 0,
            meta: raw & 8 != 0,
            caps_lock: raw & 16 != 0,
        }
    }
}

/// Size of an encoded [`KeyEvent`].
pub const KEY_EVENT_SIZE: usize = 4;

impl KeyEvent {
    /// The message encoding: key code (u16, little endian), pressed (0 or
    /// 1), modifiers.
    pub fn encode(&self) -> [u8; KEY_EVENT_SIZE] {
        let code = self.code.to_raw().to_le_bytes();
        [
            code[0],
            code[1],
            self.pressed as u8,
            self.modifiers.to_raw(),
        ]
    }

    /// Decodes the first [`KEY_EVENT_SIZE`] bytes of `bytes`.
    pub fn decode(bytes: &[u8]) -> Option<KeyEvent> {
        let b = bytes.get(..KEY_EVENT_SIZE)?;
        Some(KeyEvent {
            code: KeyCode::from_raw(u16::from_le_bytes([b[0], b[1]]))?,
            pressed: b[2] != 0,
            modifiers: Modifiers::from_raw(b[3]),
        })
    }
}

/// The letter of a letter key, lower case.
const fn letter(code: KeyCode) -> Option<u8> {
    use KeyCode::*;
    Some(match code {
        A => b'a',
        B => b'b',
        C => b'c',
        D => b'd',
        E => b'e',
        F => b'f',
        G => b'g',
        H => b'h',
        I => b'i',
        J => b'j',
        K => b'k',
        L => b'l',
        M => b'm',
        N => b'n',
        O => b'o',
        P => b'p',
        Q => b'q',
        R => b'r',
        S => b's',
        T => b't',
        U => b'u',
        V => b'v',
        W => b'w',
        X => b'x',
        Y => b'y',
        Z => b'z',
        _ => return None,
    })
}

/// The unshifted and shifted characters of the other printing keys (US).
const fn symbol(code: KeyCode) -> Option<(char, char)> {
    use KeyCode::*;
    Some(match code {
        Backquote => ('`', '~'),
        Digit1 => ('1', '!'),
        Digit2 => ('2', '@'),
        Digit3 => ('3', '#'),
        Digit4 => ('4', '$'),
        Digit5 => ('5', '%'),
        Digit6 => ('6', '^'),
        Digit7 => ('7', '&'),
        Digit8 => ('8', '*'),
        Digit9 => ('9', '('),
        Digit0 => ('0', ')'),
        Minus => ('-', '_'),
        Equal => ('=', '+'),
        LeftBracket => ('[', '{'),
        RightBracket => (']', '}'),
        Backslash => ('\\', '|'),
        Semicolon => (';', ':'),
        Quote => ('\'', '"'),
        Comma => (',', '<'),
        Period => ('.', '>'),
        Slash => ('/', '?'),
        Space => (' ', ' '),
        _ => return None,
    })
}

/// The character a key press types on a US layout: letters follow Shift
/// XOR Caps Lock; Ctrl + letter gives the control character (Ctrl-A =
/// 0x01 ... Ctrl-Z = 0x1A); Enter, Tab, Backspace and Escape give `\n`,
/// `\t`, 0x08 and 0x1B. Releases and other keys give `None`.
pub fn key_to_char(event: &KeyEvent) -> Option<char> {
    if !event.pressed {
        return None;
    }
    let m = event.modifiers;
    if let Some(lower) = letter(event.code) {
        if m.ctrl {
            return Some(char::from(lower & 0x1F));
        }
        let upper = m.shift != m.caps_lock;
        return Some(char::from(if upper {
            lower.to_ascii_uppercase()
        } else {
            lower
        }));
    }
    if let Some((plain, shifted)) = symbol(event.code) {
        return Some(if m.shift { shifted } else { plain });
    }
    match event.code {
        KeyCode::Enter => Some('\n'),
        KeyCode::Tab => Some('\t'),
        KeyCode::Backspace => Some('\u{8}'),
        KeyCode::Escape => Some('\u{1b}'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: Modifiers) -> KeyEvent {
        KeyEvent {
            code,
            pressed: true,
            modifiers,
        }
    }

    #[test]
    fn encoding_round_trip() {
        assert_eq!(KeyCode::Escape.to_raw(), 0);
        assert_eq!(KeyCode::NumLock.to_raw() as usize, KeyCode::NAMED.len() - 1);
        for (i, code) in KeyCode::NAMED.iter().enumerate() {
            assert_eq!(code.to_raw() as usize, i);
            assert_eq!(KeyCode::from_raw(i as u16), Some(*code));
        }
        assert_eq!(KeyCode::from_raw(KeyCode::NAMED.len() as u16), None);
        assert_eq!(KeyCode::Unknown(0x148).to_raw(), 0x8148);
        assert_eq!(KeyCode::from_raw(0x8148), Some(KeyCode::Unknown(0x148)));
        let m = Modifiers {
            shift: true,
            ctrl: false,
            alt: true,
            meta: false,
            caps_lock: true,
        };
        assert_eq!(m.to_raw(), 0b10101);
        let event = KeyEvent {
            code: KeyCode::Q,
            pressed: false,
            modifiers: m,
        };
        let bytes = event.encode();
        assert_eq!(KeyEvent::decode(&bytes), Some(event));
        assert_eq!(KeyEvent::decode(&bytes[..3]), None);
    }

    #[test]
    fn characters() {
        let none = Modifiers::default();
        let shift = Modifiers {
            shift: true,
            ..none
        };
        let ctrl = Modifiers { ctrl: true, ..none };
        assert_eq!(key_to_char(&press(KeyCode::A, none)), Some('a'));
        assert_eq!(key_to_char(&press(KeyCode::A, shift)), Some('A'));
        assert_eq!(key_to_char(&press(KeyCode::C, ctrl)), Some('\u{3}'));
        assert_eq!(key_to_char(&press(KeyCode::Digit1, shift)), Some('!'));
        assert_eq!(key_to_char(&press(KeyCode::Enter, none)), Some('\n'));
        assert_eq!(key_to_char(&press(KeyCode::Up, none)), None);
        let release = KeyEvent {
            pressed: false,
            ..press(KeyCode::A, none)
        };
        assert_eq!(key_to_char(&release), None);
    }
}
