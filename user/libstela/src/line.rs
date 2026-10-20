// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Line editing for text input (`docs/userland.md`): printable characters
//! are added, Backspace takes the last one away, Ctrl-U clears the line,
//! Enter ends it. The editor says what to show; the caller prints it.
//! Pure logic, tested on the host.

use alloc::string::String;

/// What a key did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Nothing to show.
    None,
    /// Show this text (a typed character).
    Echo(char),
    /// Take back this many characters on the screen.
    Erase(usize),
    /// The line is done (Enter): its text.
    Done(String),
}

/// The line being typed.
#[derive(Clone, Debug, Default)]
pub struct LineEditor {
    line: String,
    max: usize,
}

impl LineEditor {
    /// An empty line that takes at most `max` characters.
    pub fn new(max: usize) -> LineEditor {
        LineEditor {
            line: String::new(),
            max,
        }
    }

    /// The text so far.
    pub fn text(&self) -> &str {
        &self.line
    }

    /// Feeds one character (as `abi::key::key_to_char` gives it).
    pub fn feed(&mut self, c: char) -> Edit {
        match c {
            '\n' | '\r' => Edit::Done(core::mem::take(&mut self.line)),
            '\u{8}' | '\u{7f}' => match self.line.pop() {
                Some(_) => Edit::Erase(1),
                None => Edit::None,
            },
            // Ctrl-U: the whole line.
            '\u{15}' => {
                let n = self.line.chars().count();
                self.line.clear();
                if n == 0 { Edit::None } else { Edit::Erase(n) }
            }
            '\t' => self.feed(' '),
            c if c.is_control() => Edit::None,
            _ if self.line.chars().count() >= self.max => Edit::None,
            c => {
                self.line.push(c);
                Edit::Echo(c)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing() {
        let mut e = LineEditor::new(5);
        assert_eq!(e.feed('l'), Edit::Echo('l'));
        assert_eq!(e.feed('x'), Edit::Echo('x'));
        assert_eq!(e.feed('\u{8}'), Edit::Erase(1));
        assert_eq!(e.feed('s'), Edit::Echo('s'));
        assert_eq!(e.text(), "ls");
        assert_eq!(e.feed('\u{3}'), Edit::None); // Ctrl-C: ignored
        assert_eq!(e.feed('\n'), Edit::Done("ls".into()));
        assert_eq!(e.text(), "");
        assert_eq!(e.feed('\u{8}'), Edit::None);
        for c in "abcdefg".chars() {
            e.feed(c);
        }
        assert_eq!(e.text(), "abcde"); // at most 5
        assert_eq!(e.feed('\u{15}'), Edit::Erase(5));
        assert_eq!(e.feed('\u{15}'), Edit::None);
        assert_eq!(e.feed('é'), Edit::Echo('é'));
        assert_eq!(e.feed('\t'), Edit::Echo(' '));
        assert_eq!(e.feed('\n'), Edit::Done("é ".into()));
    }
}
