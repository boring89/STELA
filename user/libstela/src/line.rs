// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Line editing for text input (`docs/userland.md` §6.2): typing inserts
//! at the cursor; Left/Right, Home/End (Ctrl-A/Ctrl-E) move it;
//! Backspace and Delete take a character away, Ctrl-K the rest of the
//! line, Ctrl-U all of it; Up/Down go through the history (P11); Enter
//! ends the line. The editor says what to print to bring the screen up to
//! date, using only Backspace (moves the cursor left) and spaces, so any
//! terminal shows it. Pure logic, tested on the host.

use alloc::string::String;
use alloc::vec::Vec;

/// A key for the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A character (`abi::key::key_to_char`; control characters too).
    Char(char),
    /// Cursor left.
    Left,
    /// Cursor right.
    Right,
    /// The previous line of the history.
    Up,
    /// The next line of the history.
    Down,
    /// The start of the line.
    Home,
    /// Its end.
    End,
    /// Delete the character under the cursor.
    Delete,
}

/// What a key did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Nothing to show.
    None,
    /// Print this to update the screen.
    Show(String),
    /// The line is done (Enter): its text.
    Done(String),
}

/// Lines the history keeps.
pub const HISTORY: usize = 200;

/// The line being typed, and the lines typed before.
#[derive(Clone, Debug, Default)]
pub struct LineEditor {
    line: Vec<char>,
    cursor: usize,
    max: usize,
    history: Vec<String>,
    /// While going through the history: the entry shown, and the line
    /// that was being typed.
    browsing: Option<(usize, Vec<char>)>,
}

/// `n` backspaces.
fn back(n: usize) -> String {
    "\u{8}".repeat(n)
}

impl LineEditor {
    /// An empty line that takes at most `max` characters.
    pub fn new(max: usize) -> LineEditor {
        LineEditor {
            max,
            ..LineEditor::default()
        }
    }

    /// The text so far.
    pub fn text(&self) -> String {
        self.line.iter().collect()
    }

    /// Where the cursor is (characters from the start).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The history, oldest first.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Adds a line to the history (not an empty one, nor the same as the
    /// last); the oldest go past `HISTORY`.
    pub fn remember(&mut self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() || self.history.last().is_some_and(|l| l == line) {
            return;
        }
        self.history.push(String::from(line));
        if self.history.len() > HISTORY {
            let extra = self.history.len() - HISTORY;
            self.history.drain(..extra);
        }
    }

    /// Feeds one character (as `abi::key::key_to_char` gives it).
    pub fn feed(&mut self, c: char) -> Edit {
        self.feed_key(Key::Char(c))
    }

    /// The text from the cursor on.
    fn tail(&self) -> String {
        self.line[self.cursor..].iter().collect()
    }

    /// Replaces the whole line with `new` (the cursor at its end).
    fn replace(&mut self, new: Vec<char>) -> Edit {
        let old = self.line.len();
        let mut out = back(self.cursor);
        out.extend(new.iter());
        let extra = old.saturating_sub(new.len());
        out.push_str(&" ".repeat(extra));
        out.push_str(&back(extra));
        self.line = new;
        self.cursor = self.line.len();
        Edit::Show(out)
    }

    /// Feeds one key.
    pub fn feed_key(&mut self, key: Key) -> Edit {
        let show = |s: String| {
            if s.is_empty() {
                Edit::None
            } else {
                Edit::Show(s)
            }
        };
        match key {
            Key::Char('\n' | '\r') => {
                let line = self.text();
                self.line.clear();
                self.cursor = 0;
                self.browsing = None;
                self.remember(&line);
                Edit::Done(line)
            }
            Key::Char('\u{8}' | '\u{7f}') => {
                if self.cursor == 0 {
                    return Edit::None;
                }
                self.cursor -= 1;
                self.line.remove(self.cursor);
                let tail = self.tail();
                let n = tail.chars().count();
                show(alloc::format!("\u{8}{tail} {}", back(n + 1)))
            }
            Key::Delete => {
                if self.cursor >= self.line.len() {
                    return Edit::None;
                }
                self.line.remove(self.cursor);
                let tail = self.tail();
                let n = tail.chars().count();
                show(alloc::format!("{tail} {}", back(n + 1)))
            }
            // Ctrl-U: the whole line.
            Key::Char('\u{15}') => self.replace(Vec::new()),
            // Ctrl-K: from the cursor on.
            Key::Char('\u{b}') => {
                let n = self.line.len() - self.cursor;
                self.line.truncate(self.cursor);
                show(alloc::format!("{}{}", " ".repeat(n), back(n)))
            }
            Key::Home | Key::Char('\u{1}') => {
                let n = self.cursor;
                self.cursor = 0;
                show(back(n))
            }
            Key::End | Key::Char('\u{5}') => {
                let tail = self.tail();
                self.cursor = self.line.len();
                show(tail)
            }
            Key::Left => {
                if self.cursor == 0 {
                    return Edit::None;
                }
                self.cursor -= 1;
                show(back(1))
            }
            Key::Right => {
                let Some(&c) = self.line.get(self.cursor) else {
                    return Edit::None;
                };
                self.cursor += 1;
                show(String::from(c))
            }
            Key::Up => {
                let at = match &self.browsing {
                    Some((i, _)) if *i == 0 => return Edit::None,
                    Some((i, _)) => i - 1,
                    None if self.history.is_empty() => return Edit::None,
                    None => {
                        self.browsing = Some((self.history.len(), self.line.clone()));
                        self.history.len() - 1
                    }
                };
                if let Some((i, _)) = &mut self.browsing {
                    *i = at;
                }
                let new: Vec<char> = self.history[at].chars().take(self.max).collect();
                self.replace(new)
            }
            Key::Down => {
                let Some((i, saved)) = self.browsing.clone() else {
                    return Edit::None;
                };
                if i + 1 >= self.history.len() {
                    // Past the newest: the line that was being typed.
                    self.browsing = None;
                    return self.replace(saved);
                }
                self.browsing = Some((i + 1, saved));
                let new: Vec<char> = self.history[i + 1].chars().take(self.max).collect();
                self.replace(new)
            }
            Key::Char('\t') => self.feed_key(Key::Char(' ')),
            Key::Char(c) if c.is_control() => Edit::None,
            Key::Char(_) if self.line.len() >= self.max => Edit::None,
            Key::Char(c) => {
                self.line.insert(self.cursor, c);
                self.cursor += 1;
                let tail = self.tail();
                let n = tail.chars().count();
                show(alloc::format!("{c}{tail}{}", back(n)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a terminal shows after printing `out`: (line, cursor).
    fn screen(out: &[Edit]) -> (String, usize) {
        let mut cells: Vec<char> = Vec::new();
        let mut at: usize = 0;
        for e in out {
            if let Edit::Show(s) = e {
                for c in s.chars() {
                    if c == '\u{8}' {
                        at = at.saturating_sub(1);
                    } else {
                        if at == cells.len() {
                            cells.push(c);
                        } else {
                            cells[at] = c;
                        }
                        at += 1;
                    }
                }
            }
        }
        (cells.iter().collect::<String>().trim_end().into(), at)
    }

    fn keys(e: &mut LineEditor, ks: &[Key]) -> Vec<Edit> {
        ks.iter().map(|&k| e.feed_key(k)).collect()
    }

    fn typed(s: &str) -> Vec<Key> {
        s.chars().map(Key::Char).collect()
    }

    #[test]
    fn editing() {
        let mut e = LineEditor::new(5);
        assert_eq!(e.feed('l'), Edit::Show("l".into()));
        assert_eq!(e.feed('x'), Edit::Show("x".into()));
        assert_eq!(e.feed('\u{8}'), Edit::Show("\u{8} \u{8}".into()));
        e.feed('s');
        assert_eq!(e.text(), "ls");
        assert_eq!(e.feed('\u{3}'), Edit::None); // Ctrl-C: ignored
        assert_eq!(e.feed('\n'), Edit::Done("ls".into()));
        assert_eq!(e.text(), "");
        assert_eq!(e.feed('\u{8}'), Edit::None);
        for c in "abcdefg".chars() {
            e.feed(c);
        }
        assert_eq!(e.text(), "abcde"); // at most 5
        e.feed('\u{15}');
        assert_eq!(e.text(), "");
        assert_eq!(e.feed('é'), Edit::Show("é".into()));
        assert_eq!(e.feed('\t'), Edit::Show(" ".into()));
        assert_eq!(e.feed('\n'), Edit::Done("é ".into()));
    }

    #[test]
    fn the_cursor_moves_and_text_goes_in_between() {
        let mut e = LineEditor::new(80);
        let mut out = keys(&mut e, &typed("lsData"));
        out.extend(keys(&mut e, &[Key::Left, Key::Left, Key::Left, Key::Left]));
        out.extend(keys(&mut e, &typed(" ")));
        assert_eq!(e.text(), "ls Data");
        assert_eq!(screen(&out), (String::from("ls Data"), 3));
        // Home, Delete, End; Ctrl-A, Ctrl-K.
        out.extend(keys(&mut e, &[Key::Home, Key::Delete, Key::End]));
        assert_eq!(e.text(), "s Data");
        assert_eq!(screen(&out), (String::from("s Data"), 6));
        out.extend(keys(
            &mut e,
            &[Key::Char('\u{1}'), Key::Right, Key::Char('\u{b}')],
        ));
        assert_eq!(e.text(), "s");
        assert_eq!(screen(&out), (String::from("s"), 1));
        // Backspace in the middle.
        let mut e = LineEditor::new(80);
        let mut out = keys(&mut e, &typed("abcd"));
        out.extend(keys(&mut e, &[Key::Left, Key::Char('\u{8}')]));
        assert_eq!(e.text(), "abd");
        assert_eq!(screen(&out), (String::from("abd"), 2));
        assert_eq!(e.feed_key(Key::Right), Edit::Show("d".into()));
        assert_eq!(e.feed_key(Key::Right), Edit::None);
    }

    #[test]
    fn history_up_and_down() {
        let mut e = LineEditor::new(80);
        assert_eq!(e.feed_key(Key::Up), Edit::None);
        for line in ["ls", "cd t", "cd t", "", "pwd"] {
            keys(&mut e, &typed(line));
            e.feed('\n');
        }
        assert_eq!(e.history(), ["ls", "cd t", "pwd"]);
        let mut out = keys(&mut e, &typed("half"));
        out.extend(keys(&mut e, &[Key::Up]));
        assert_eq!(e.text(), "pwd");
        out.extend(keys(&mut e, &[Key::Up, Key::Up, Key::Up]));
        assert_eq!(e.text(), "ls");
        assert_eq!(screen(&out), (String::from("ls"), 2));
        out.extend(keys(&mut e, &[Key::Down]));
        assert_eq!(e.text(), "cd t");
        out.extend(keys(&mut e, &[Key::Down, Key::Down]));
        assert_eq!(e.text(), "half");
        assert_eq!(screen(&out), (String::from("half"), 4));
        assert_eq!(e.feed_key(Key::Down), Edit::None);
        for i in 0..250 {
            e.remember(&alloc::format!("line {i}"));
        }
        assert_eq!(e.history().len(), HISTORY);
        assert_eq!(e.history()[0], "line 50");
    }
}
