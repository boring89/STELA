// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The viewer's line wrapping (`wrap`), a library so that `cargo test
//! --lib` runs it on the host; the program is `main.rs`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// The text as display lines of at most `width` characters: long lines
/// wrap, tabs become spaces up to the next multiple of 8, control
/// characters are dropped, invalid UTF-8 shows as U+FFFD.
pub fn wrap(bytes: &[u8], width: usize) -> Vec<String> {
    let width = width.max(1);
    let text = String::from_utf8_lossy(bytes);
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        let mut col = 0;
        for c in raw.chars() {
            if c == '\t' {
                let next = (col / 8 + 1) * 8;
                while col < next {
                    if col == width {
                        lines.push(core::mem::take(&mut line));
                        col = 0;
                        break;
                    }
                    line.push(' ');
                    col += 1;
                }
                continue;
            }
            if c.is_control() {
                continue;
            }
            if col == width {
                lines.push(core::mem::take(&mut line));
                col = 0;
            }
            line.push(c);
            col += 1;
        }
        lines.push(line);
    }
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping() {
        assert_eq!(wrap(b"abc\ndef\n", 10), ["abc", "def"]);
        assert_eq!(wrap(b"abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap(b"a\tb", 20), ["a       b"]);
        assert_eq!(wrap(b"x\r\n\n", 5), ["x", ""]);
        assert_eq!(wrap(&[0xFF, b'a'], 5), ["\u{FFFD}a"]);
        assert_eq!(wrap(b"", 5), [""]);
    }
}
