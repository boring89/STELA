// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard events: STELA's own key names ([`KeyCode`], US layout), the
//! PS/2 scan code set 1 decoder and the US character mapping. Pure logic,
//! tested on the host.
//!
//! Scan code set 1 (OSDev Wiki "PS/2 Keyboard", "Scan Code Set 1"): each key
//! sends a one-byte make code when pressed and the same code with bit 7 set
//! (break code) when released. Keys added after the PC/XT are prefixed with
//! `0xE0`; Pause sends `E1 1D 45 E1 9D C5` and nothing on release. The
//! i8042 translates the keyboard's native set 2 into set 1 by default, so
//! this is what arrives at port 0x60.

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

/// Keys without the `0xE0` prefix, indexed by make code (0x00-0x58).
const PLAIN: [Option<KeyCode>; 0x59] = {
    use KeyCode::*;
    let mut t = [None; 0x59];
    t[0x01] = Some(Escape);
    t[0x02] = Some(Digit1);
    t[0x03] = Some(Digit2);
    t[0x04] = Some(Digit3);
    t[0x05] = Some(Digit4);
    t[0x06] = Some(Digit5);
    t[0x07] = Some(Digit6);
    t[0x08] = Some(Digit7);
    t[0x09] = Some(Digit8);
    t[0x0A] = Some(Digit9);
    t[0x0B] = Some(Digit0);
    t[0x0C] = Some(Minus);
    t[0x0D] = Some(Equal);
    t[0x0E] = Some(Backspace);
    t[0x0F] = Some(Tab);
    t[0x10] = Some(Q);
    t[0x11] = Some(W);
    t[0x12] = Some(E);
    t[0x13] = Some(R);
    t[0x14] = Some(T);
    t[0x15] = Some(Y);
    t[0x16] = Some(U);
    t[0x17] = Some(I);
    t[0x18] = Some(O);
    t[0x19] = Some(P);
    t[0x1A] = Some(LeftBracket);
    t[0x1B] = Some(RightBracket);
    t[0x1C] = Some(Enter);
    t[0x1D] = Some(LeftCtrl);
    t[0x1E] = Some(A);
    t[0x1F] = Some(S);
    t[0x20] = Some(D);
    t[0x21] = Some(F);
    t[0x22] = Some(G);
    t[0x23] = Some(H);
    t[0x24] = Some(J);
    t[0x25] = Some(K);
    t[0x26] = Some(L);
    t[0x27] = Some(Semicolon);
    t[0x28] = Some(Quote);
    t[0x29] = Some(Backquote);
    t[0x2A] = Some(LeftShift);
    t[0x2B] = Some(Backslash);
    t[0x2C] = Some(Z);
    t[0x2D] = Some(X);
    t[0x2E] = Some(C);
    t[0x2F] = Some(V);
    t[0x30] = Some(B);
    t[0x31] = Some(N);
    t[0x32] = Some(M);
    t[0x33] = Some(Comma);
    t[0x34] = Some(Period);
    t[0x35] = Some(Slash);
    t[0x36] = Some(RightShift);
    t[0x38] = Some(LeftAlt);
    t[0x39] = Some(Space);
    t[0x3A] = Some(CapsLock);
    t[0x3B] = Some(F1);
    t[0x3C] = Some(F2);
    t[0x3D] = Some(F3);
    t[0x3E] = Some(F4);
    t[0x3F] = Some(F5);
    t[0x40] = Some(F6);
    t[0x41] = Some(F7);
    t[0x42] = Some(F8);
    t[0x43] = Some(F9);
    t[0x44] = Some(F10);
    t[0x45] = Some(NumLock);
    t[0x46] = Some(ScrollLock);
    t[0x57] = Some(F11);
    t[0x58] = Some(F12);
    t
};

/// Keys with the `0xE0` prefix.
const fn extended_key(code: u8) -> Option<KeyCode> {
    use KeyCode::*;
    Some(match code {
        0x1D => RightCtrl,
        0x37 => PrintScreen,
        0x38 => RightAlt,
        0x47 => Home,
        0x48 => Up,
        0x49 => PageUp,
        0x4B => Left,
        0x4D => Right,
        0x4F => End,
        0x50 => Down,
        0x51 => PageDown,
        0x52 => Insert,
        0x53 => Delete,
        0x5B => LeftMeta,
        0x5C => RightMeta,
        0x5D => Menu,
        _ => return None,
    })
}

/// Turns a stream of set-1 scan code bytes into [`KeyEvent`]s and tracks
/// the modifier keys.
#[derive(Clone, Debug, Default)]
pub struct Decoder {
    /// The previous byte was `0xE0`.
    extended: bool,
    /// Bytes of the Pause sequence still to swallow after `0xE1`.
    pause_left: u8,
    left_shift: bool,
    right_shift: bool,
    left_ctrl: bool,
    right_ctrl: bool,
    left_alt: bool,
    right_alt: bool,
    left_meta: bool,
    right_meta: bool,
    caps_lock: bool,
}

impl Decoder {
    /// A decoder with no keys down and Caps Lock off.
    pub const fn new() -> Decoder {
        Decoder {
            extended: false,
            pause_left: 0,
            left_shift: false,
            right_shift: false,
            left_ctrl: false,
            right_ctrl: false,
            left_alt: false,
            right_alt: false,
            left_meta: false,
            right_meta: false,
            caps_lock: false,
        }
    }

    /// The current modifier state.
    pub fn modifiers(&self) -> Modifiers {
        Modifiers {
            shift: self.left_shift || self.right_shift,
            ctrl: self.left_ctrl || self.right_ctrl,
            alt: self.left_alt || self.right_alt,
            meta: self.left_meta || self.right_meta,
            caps_lock: self.caps_lock,
        }
    }

    /// Feeds one byte from the keyboard; returns an event when a key
    /// press or release is complete.
    pub fn feed(&mut self, byte: u8) -> Option<KeyEvent> {
        if self.pause_left > 0 {
            self.pause_left -= 1;
            if self.pause_left > 0 {
                return None;
            }
            return Some(self.event(KeyCode::Pause, true));
        }
        match byte {
            0xE0 => {
                self.extended = true;
                return None;
            }
            0xE1 => {
                self.pause_left = 5;
                return None;
            }
            // Keyboard replies (ACK, resend, echo, errors) that can show up
            // in the data stream. Not 0xAA (self-test passed): in set 1
            // that is also the release of Left Shift.
            0x00 | 0xEE | 0xFA | 0xFC | 0xFD | 0xFE | 0xFF => {
                self.extended = false;
                return None;
            }
            _ => {}
        }
        let extended = core::mem::take(&mut self.extended);
        let pressed = byte & 0x80 == 0;
        let make = byte & 0x7F;
        let code = if extended {
            // E0 2A / E0 AA (and E0 36 / E0 B6) are "fake shifts" around
            // PrintScreen and the navigation keys: ignore them.
            if make == 0x2A || make == 0x36 {
                return None;
            }
            extended_key(make).unwrap_or(KeyCode::Unknown(0x100 | u16::from(make)))
        } else {
            PLAIN
                .get(usize::from(make))
                .copied()
                .flatten()
                .unwrap_or(KeyCode::Unknown(u16::from(make)))
        };
        self.update_modifiers(code, pressed);
        Some(self.event(code, pressed))
    }

    fn update_modifiers(&mut self, code: KeyCode, pressed: bool) {
        use KeyCode::*;
        match code {
            LeftShift => self.left_shift = pressed,
            RightShift => self.right_shift = pressed,
            LeftCtrl => self.left_ctrl = pressed,
            RightCtrl => self.right_ctrl = pressed,
            LeftAlt => self.left_alt = pressed,
            RightAlt => self.right_alt = pressed,
            LeftMeta => self.left_meta = pressed,
            RightMeta => self.right_meta = pressed,
            CapsLock if pressed => self.caps_lock = !self.caps_lock,
            _ => {}
        }
    }

    fn event(&self, code: KeyCode, pressed: bool) -> KeyEvent {
        KeyEvent {
            code,
            pressed,
            modifiers: self.modifiers(),
        }
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

    fn feed_all(decoder: &mut Decoder, bytes: &[u8]) -> Vec<KeyEvent> {
        bytes.iter().filter_map(|&b| decoder.feed(b)).collect()
    }

    fn typed(bytes: &[u8]) -> String {
        let mut decoder = Decoder::new();
        feed_all(&mut decoder, bytes)
            .iter()
            .filter_map(key_to_char)
            .collect()
    }

    #[test]
    fn make_and_break() {
        let mut d = Decoder::new();
        let events = feed_all(&mut d, &[0x1F, 0x9F]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].code, KeyCode::S);
        assert!(events[0].pressed);
        assert_eq!(events[1].code, KeyCode::S);
        assert!(!events[1].pressed);
    }

    #[test]
    fn types_stela() {
        // s t e l a Enter, each pressed and released.
        let bytes = [
            0x1F, 0x9F, 0x14, 0x94, 0x12, 0x92, 0x26, 0xA6, 0x1E, 0x9E, 0x1C, 0x9C,
        ];
        assert_eq!(typed(&bytes), "stela\n");
    }

    #[test]
    fn shift_and_caps_lock() {
        // Shift down, a, 1, Shift up, a.
        assert_eq!(typed(&[0x2A, 0x1E, 0x9E, 0x02, 0x82, 0xAA, 0x1E]), "A!a");
        // Right Shift works too.
        assert_eq!(typed(&[0x36, 0x35, 0xB6, 0x35]), "?/");
        // Caps Lock toggles letters only; Shift inverts it.
        assert_eq!(
            typed(&[0x3A, 0xBA, 0x1E, 0x02, 0x2A, 0x1E, 0xAA, 0x3A, 0xBA, 0x1E]),
            "A1aa"
        );
    }

    #[test]
    fn ctrl_letters_are_control_characters() {
        // Left Ctrl + C, then Right Ctrl (E0 1D) + A.
        assert_eq!(typed(&[0x1D, 0x2E, 0x9D, 0xE0, 0x1D, 0x1E]), "\u{3}\u{1}");
    }

    #[test]
    fn extended_keys() {
        let mut d = Decoder::new();
        let events = feed_all(&mut d, &[0xE0, 0x48, 0xE0, 0xC8, 0xE0, 0x53, 0xE0, 0x38]);
        let codes: Vec<_> = events.iter().map(|e| (e.code, e.pressed)).collect();
        assert_eq!(
            codes,
            [
                (KeyCode::Up, true),
                (KeyCode::Up, false),
                (KeyCode::Delete, true),
                (KeyCode::RightAlt, true)
            ]
        );
        assert!(events[3].modifiers.alt);
        // The prefix applies to one byte only: 0x48 alone is keypad 8.
        assert_eq!(d.feed(0x48).map(|e| e.code), Some(KeyCode::Unknown(0x48)));
    }

    #[test]
    fn print_screen_and_pause() {
        let mut d = Decoder::new();
        // PrintScreen: E0 2A E0 37, release E0 B7 E0 AA.
        let events = feed_all(&mut d, &[0xE0, 0x2A, 0xE0, 0x37, 0xE0, 0xB7, 0xE0, 0xAA]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].code, KeyCode::PrintScreen);
        assert!(!d.modifiers().shift);
        // Pause: E1 1D 45 E1 9D C5, one press event, nothing else.
        let events = feed_all(&mut d, &[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].code, KeyCode::Pause);
        assert!(!d.modifiers().ctrl);
    }

    #[test]
    fn replies_are_ignored() {
        let mut d = Decoder::new();
        assert!(feed_all(&mut d, &[0xFA, 0xFE, 0x00, 0xFF]).is_empty());
    }

    #[test]
    fn special_characters() {
        // Enter, Tab, Backspace, Escape, Space.
        assert_eq!(typed(&[0x1C, 0x0F, 0x0E, 0x01, 0x39]), "\n\t\u{8}\u{1b} ");
        // F1, arrows and releases type nothing.
        assert_eq!(typed(&[0x3B, 0xE0, 0x4B, 0x9E]), "");
    }
}
