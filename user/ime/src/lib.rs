// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The input method (`docs/ime.md`): Zhuyin (Bopomofo) with libchewing
//! (C, built by `build.rs`, wrapped by [`ffi`], the only module with
//! `unsafe` code). [`Ime`] turns key presses into what the window server
//! shows and sends on: committed text, the text being composed and the
//! candidate list. The program (`main.rs`) is the service the window
//! server talks to; this library runs on the host (`cargo test --lib`).

#![cfg_attr(not(test), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

pub mod ffi;

use abi::key::{KeyCode, KeyEvent};
use alloc::string::String;
use alloc::vec::Vec;
use ffi::{Chewing, InitError, Key};

/// The dictionary files libchewing reads, as they sit in
/// `Initrd:/chewing/` (built by `make chewing`).
pub const DICT_FILES: [&str; 5] = [
    "dictionary.dat",
    "index_tree.dat",
    "pinyin.tab",
    "swkb.dat",
    "symbols.dat",
];

/// Dictionary files: names (see [`DICT_FILES`]) and contents.
pub type Files<'a> = Vec<(&'a str, Vec<u8>)>;

/// What a key did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The IME took the key (otherwise it goes to the application as it
    /// is).
    pub consumed: bool,
    /// Text to give the application (UTF-8, may be empty).
    pub commit: String,
}

/// The composition the window server draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composition {
    /// The text being composed, the Zhuyin of the syllable being typed
    /// included (at the cursor).
    pub preedit: String,
    /// The cursor, in characters of `preedit`.
    pub cursor: usize,
    /// The candidates of the open list's page (empty: no list).
    pub candidates: Vec<String>,
    /// The page shown and the number of pages.
    pub page: usize,
    /// The number of pages.
    pub pages: usize,
    /// Chinese (true) or English mode.
    pub chinese: bool,
}

/// The input method.
pub struct Ime {
    chewing: Chewing,
}

/// libchewing's key for a key press, when it has one.
fn chewing_key(event: &KeyEvent) -> Option<Key> {
    let m = event.modifiers;
    if m.ctrl {
        let digit = match event.code {
            KeyCode::Digit0 => 0,
            KeyCode::Digit1 => 1,
            KeyCode::Digit2 => 2,
            KeyCode::Digit3 => 3,
            KeyCode::Digit4 => 4,
            KeyCode::Digit5 => 5,
            KeyCode::Digit6 => 6,
            KeyCode::Digit7 => 7,
            KeyCode::Digit8 => 8,
            KeyCode::Digit9 => 9,
            _ => return None,
        };
        return Some(Key::CtrlDigit(digit));
    }
    if m.alt || m.meta {
        return None;
    }
    Some(match event.code {
        KeyCode::Space if m.shift => Key::ShiftSpace,
        KeyCode::Space => Key::Space,
        KeyCode::Enter => Key::Enter,
        KeyCode::Escape => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Tab => Key::Tab,
        _ => {
            let c = abi::key::key_to_char(event)?;
            if !c.is_ascii_graphic() {
                return None;
            }
            Key::Char(c as u8)
        }
    })
}

/// Whether a key press is Ctrl-Space (Chinese / English).
pub fn is_toggle(event: &KeyEvent) -> bool {
    event.pressed
        && event.code == KeyCode::Space
        && event.modifiers.ctrl
        && !event.modifiers.alt
        && !event.modifiers.shift
}

impl Ime {
    /// Starts the IME with the dictionary files (name and contents, see
    /// [`DICT_FILES`]) and the user's phrases (empty for none).
    pub fn new(files: Files<'_>, user: &[u8]) -> Result<Ime, InitError> {
        Ok(Ime {
            chewing: Chewing::new(files, user)?,
        })
    }

    /// Whether something is being composed.
    pub fn composing(&self) -> bool {
        let (buf, zhuyin, _) = self.chewing.preedit();
        !buf.is_empty() || !zhuyin.is_empty()
    }

    /// Chinese (true) or English mode.
    pub fn chinese(&self) -> bool {
        self.chewing.chinese()
    }

    /// Switches between Chinese and English (Ctrl-Space); what is being
    /// composed is committed first.
    pub fn toggle(&mut self) -> Outcome {
        let mut out = Outcome {
            consumed: true,
            commit: String::new(),
        };
        if self.composing() {
            // Enter commits the buffer (an open candidate list is
            // closed by Esc first).
            if !self.chewing.candidates().0.is_empty() {
                self.chewing.key(Key::Esc);
            }
            self.chewing.key(Key::Enter);
            out.commit = self.chewing.commit();
        }
        let on = !self.chewing.chinese();
        self.chewing.set_chinese(on);
        out
    }

    /// Handles a key event.
    pub fn key(&mut self, event: &KeyEvent) -> Outcome {
        let pass = Outcome::default();
        if !event.pressed {
            // Releases go to the application, except while composing.
            return Outcome {
                consumed: self.composing(),
                commit: String::new(),
            };
        }
        if is_toggle(event) {
            return self.toggle();
        }
        let composing = self.composing();
        let Some(key) = chewing_key(event) else {
            // Shortcuts (Alt, Ctrl) and keys libchewing has no use for:
            // swallowed while composing, the application's otherwise.
            return Outcome {
                consumed: composing,
                commit: String::new(),
            };
        };
        if !composing {
            // Nothing to edit: editing keys, and every key in English
            // mode, belong to the application.
            let typing = matches!(key, Key::Char(_) | Key::Space | Key::ShiftSpace);
            if !self.chewing.chinese() || !typing {
                return pass;
            }
        }
        let taken = self.chewing.key(key);
        let commit = self.chewing.commit();
        Outcome {
            consumed: taken || !commit.is_empty() || composing,
            commit,
        }
    }

    /// What is being composed now.
    pub fn composition(&mut self) -> Composition {
        let (buf, zhuyin, cursor) = self.chewing.preedit();
        let at = buf
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(buf.len());
        let mut preedit = String::from(&buf[..at]);
        preedit.push_str(&zhuyin);
        preedit.push_str(&buf[at..]);
        let (candidates, page, pages) = self.chewing.candidates();
        Composition {
            preedit,
            cursor: cursor + zhuyin.chars().count(),
            candidates,
            page,
            pages,
            chinese: self.chewing.chinese(),
        }
    }

    /// The user-phrase file when it changed since the last call.
    pub fn changed_user_data(&mut self) -> Option<Vec<u8>> {
        self.chewing.changed_user_data()
    }
}

#[cfg(test)]
mod tests;
