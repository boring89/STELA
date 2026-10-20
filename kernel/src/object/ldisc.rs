// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The line discipline of a terminal (tty): what happens to typed
//! characters before a program reads them. Pure logic, tested on the host;
//! `tty.rs` wraps it in a kernel object.
//!
//! ```text
//!  canonical (the default)            raw (ICANON cleared)
//!  ───────────────────────            ────────────────────
//!  h e l l o ⌫ ⏎                      every character is readable at once
//!  └ edited in `line`, echoed ┘
//!  "hell\n" becomes readable          Ctrl-C still interrupts (ISIG)
//!  ⌫ (0x08, 0x7F) erases a character
//!  Ctrl-U erases the line
//!  Ctrl-D on an empty line: end of file (a read returns 0 bytes)
//!  Ctrl-C: the line is dropped, "^C" echoed, the program interrupted
//! ```
//!
//! (POSIX "General Terminal Interface", 11.1.6 "Canonical Mode Input
//! Processing".)

use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Longest line kept in canonical mode (Linux N_TTY: 4096).
pub const LINE_MAX: usize = 4096;
/// Most bytes waiting to be read.
pub const READY_MAX: usize = 16 * 1024;

const CTRL_C: char = '\u{3}';
const CTRL_D: char = '\u{4}';
const CTRL_U: char = '\u{15}';
const BACKSPACE: char = '\u{8}';
const DELETE: char = '\u{7F}';

/// What feeding a character did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fed {
    /// Bytes to show on the terminal (the echo).
    pub echo: Vec<u8>,
    /// Ctrl-C: interrupt the programs using the terminal.
    pub interrupt: bool,
    /// Something became readable.
    pub readable: bool,
}

/// The state of one terminal's input.
#[derive(Debug)]
pub struct LineDiscipline {
    /// ICANON: line editing.
    pub canonical: bool,
    /// ECHO: typed characters are shown.
    pub echo: bool,
    /// ISIG: Ctrl-C interrupts.
    pub signals: bool,
    line: Vec<u8>,
    ready: VecDeque<u8>,
    /// End-of-file marks (Ctrl-D on an empty line) not yet read.
    eof: usize,
}

impl Default for LineDiscipline {
    fn default() -> Self {
        Self::new()
    }
}

impl LineDiscipline {
    /// Canonical mode with echo and signals (the usual terminal).
    pub const fn new() -> LineDiscipline {
        LineDiscipline {
            canonical: true,
            echo: true,
            signals: true,
            line: Vec::new(),
            ready: VecDeque::new(),
            eof: 0,
        }
    }

    fn push_ready(&mut self, bytes: &[u8]) {
        let room = READY_MAX.saturating_sub(self.ready.len());
        self.ready.extend(&bytes[..bytes.len().min(room)]);
    }

    /// Handles one typed character.
    pub fn feed(&mut self, c: char) -> Fed {
        let mut fed = Fed::default();
        if c == CTRL_C && self.signals {
            self.line.clear();
            if self.echo {
                fed.echo.extend_from_slice(b"^C\n");
            }
            fed.interrupt = true;
            return fed;
        }
        let mut utf8 = [0u8; 4];
        let bytes = c.encode_utf8(&mut utf8).as_bytes();
        if !self.canonical {
            self.push_ready(bytes);
            fed.readable = true;
            if self.echo {
                fed.echo.extend_from_slice(bytes);
            }
            return fed;
        }
        match c {
            '\r' | '\n' => {
                self.line.push(b'\n');
                let line = core::mem::take(&mut self.line);
                self.push_ready(&line);
                fed.readable = true;
                if self.echo {
                    fed.echo.push(b'\n');
                }
            }
            BACKSPACE | DELETE => {
                if self.erase_char() && self.echo {
                    fed.echo.extend_from_slice(b"\x08 \x08");
                }
            }
            CTRL_U => {
                let n =
                    core::str::from_utf8(&self.line).map_or(self.line.len(), |s| s.chars().count());
                self.line.clear();
                if self.echo {
                    for _ in 0..n {
                        fed.echo.extend_from_slice(b"\x08 \x08");
                    }
                }
            }
            CTRL_D => {
                if self.line.is_empty() {
                    self.eof += 1;
                } else {
                    let line = core::mem::take(&mut self.line);
                    self.push_ready(&line);
                }
                fed.readable = true;
            }
            c if c.is_control() && c != '\t' => {}
            _ => {
                if self.line.len() + bytes.len() < LINE_MAX {
                    self.line.extend_from_slice(bytes);
                    if self.echo {
                        fed.echo.extend_from_slice(bytes);
                    }
                }
            }
        }
        fed
    }

    /// Removes the last character of the line; whether there was one.
    fn erase_char(&mut self) -> bool {
        let Some(&last) = self.line.last() else {
            return false;
        };
        self.line.pop();
        if last >= 0x80 {
            // Drop continuation bytes up to and including the lead byte.
            while let Some(&b) = self.line.last() {
                self.line.pop();
                if b & 0xC0 != 0x80 {
                    break;
                }
            }
        }
        true
    }

    /// Whether a read would return now (data or an end of file).
    pub fn readable(&self) -> bool {
        !self.ready.is_empty() || self.eof > 0
    }

    /// Takes up to `max` readable bytes: in canonical mode at most one
    /// line. `Some(empty)` is an end of file; `None`: nothing yet.
    pub fn read(&mut self, max: usize) -> Option<Vec<u8>> {
        if self.ready.is_empty() {
            if self.eof > 0 {
                self.eof -= 1;
                return Some(Vec::new());
            }
            return None;
        }
        let mut out = Vec::new();
        while out.len() < max {
            let Some(b) = self.ready.pop_front() else {
                break;
            };
            out.push(b);
            if self.canonical && b == b'\n' {
                break;
            }
        }
        Some(out)
    }

    /// Drops everything typed and not read.
    pub fn flush(&mut self) {
        self.line.clear();
        self.ready.clear();
        self.eof = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_str(l: &mut LineDiscipline, s: &str) -> Vec<u8> {
        let mut echo = Vec::new();
        for c in s.chars() {
            echo.extend(l.feed(c).echo);
        }
        echo
    }

    #[test]
    fn canonical_lines() {
        let mut l = LineDiscipline::new();
        assert_eq!(feed_str(&mut l, "hi"), b"hi");
        assert!(!l.readable());
        assert_eq!(l.read(100), None);
        let fed = l.feed('\r');
        assert!(fed.readable);
        assert_eq!(fed.echo, b"\n");
        feed_str(&mut l, "two\n");
        assert_eq!(l.read(100).unwrap(), b"hi\n");
        assert_eq!(l.read(2).unwrap(), b"tw");
        assert_eq!(l.read(100).unwrap(), b"o\n");
        assert_eq!(l.read(100), None);
    }

    #[test]
    fn editing() {
        let mut l = LineDiscipline::new();
        assert_eq!(feed_str(&mut l, "ab\u{8}"), b"ab\x08 \x08");
        assert_eq!(feed_str(&mut l, "\u{7F}\u{7F}"), b"\x08 \x08");
        feed_str(&mut l, "xyz\u{15}");
        feed_str(&mut l, "é\u{8}ok\n");
        assert_eq!(l.read(100).unwrap(), b"ok\n");
        // Other control characters are ignored; tabs are kept.
        feed_str(&mut l, "a\u{1}\tb\n");
        assert_eq!(l.read(100).unwrap(), b"a\tb\n");
    }

    #[test]
    fn interrupt_and_eof() {
        let mut l = LineDiscipline::new();
        feed_str(&mut l, "half");
        let fed = l.feed('\u{3}');
        assert!(fed.interrupt);
        assert_eq!(fed.echo, b"^C\n");
        feed_str(&mut l, "\n");
        assert_eq!(l.read(10).unwrap(), b"\n");
        l.feed('\u{4}');
        assert_eq!(l.read(10).unwrap(), b"");
        assert_eq!(l.read(10), None);
        feed_str(&mut l, "ab\u{4}");
        assert_eq!(l.read(10).unwrap(), b"ab");
        l.signals = false;
        assert!(!l.feed('\u{3}').interrupt);
    }

    #[test]
    fn raw_mode() {
        let mut l = LineDiscipline::new();
        l.canonical = false;
        l.echo = false;
        let fed = l.feed('x');
        assert!(fed.readable && fed.echo.is_empty());
        l.feed('\n');
        l.feed('y');
        assert_eq!(l.read(10).unwrap(), b"x\ny");
        assert!(l.feed('\u{3}').interrupt);
        l.feed('z');
        l.flush();
        assert_eq!(l.read(10), None);
    }

    #[test]
    fn limits() {
        let mut l = LineDiscipline::new();
        for _ in 0..LINE_MAX + 10 {
            l.feed('a');
        }
        l.feed('\n');
        assert_eq!(l.read(usize::MAX).unwrap().len(), LINE_MAX);
    }
}
