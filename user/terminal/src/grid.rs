// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A grid of character cells fed with a program's output (UTF-8 bytes,
//! possibly split anywhere): printable characters advance the cursor and
//! wrap; `\n` goes to the next line (scrolling at the bottom), `\r` to the
//! start of the line, backspace one cell back, tab to the next multiple
//! of 8. Escape sequences (`ESC [ ... final`) are skipped. Rows that
//! changed are remembered for redrawing. Pure logic, tested on the host.

use alloc::vec;
use alloc::vec::Vec;

/// The grid.
#[derive(Debug)]
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<char>,
    /// The cursor's column and row.
    pub cursor: (usize, usize),
    dirty: Vec<bool>,
    /// Bytes of an unfinished UTF-8 sequence.
    partial: Vec<u8>,
    /// Inside an escape sequence: 1 after ESC, 2 after `ESC [`.
    escape: u8,
}

impl Grid {
    /// An empty grid of `cols` x `rows` (at least 1 x 1).
    pub fn new(cols: usize, rows: usize) -> Grid {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Grid {
            cols,
            rows,
            cells: vec![' '; cols * rows],
            cursor: (0, 0),
            dirty: vec![true; rows],
            partial: Vec::new(),
            escape: 0,
        }
    }

    /// Columns and rows.
    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// The character at (col, row).
    pub fn at(&self, col: usize, row: usize) -> char {
        self.cells[row * self.cols + col]
    }

    /// One row as text.
    pub fn row(&self, row: usize) -> &[char] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    fn mark(&mut self, row: usize) {
        if let Some(d) = self.dirty.get_mut(row) {
            *d = true;
        }
    }

    fn newline(&mut self) {
        self.cursor.0 = 0;
        if self.cursor.1 + 1 < self.rows {
            self.cursor.1 += 1;
        } else {
            self.cells.copy_within(self.cols.., 0);
            let last = (self.rows - 1) * self.cols;
            self.cells[last..].fill(' ');
            self.dirty.fill(true);
        }
    }

    fn put(&mut self, c: char) {
        if self.cursor.0 >= self.cols {
            self.newline();
        }
        let (x, y) = self.cursor;
        self.cells[y * self.cols + x] = c;
        self.mark(y);
        self.cursor.0 += 1;
    }

    fn control(&mut self, c: char) {
        let row = self.cursor.1;
        match c {
            '\n' => self.newline(),
            '\r' => self.cursor.0 = 0,
            '\u{8}' => self.cursor.0 = self.cursor.0.min(self.cols).saturating_sub(1),
            '\t' => {
                let next = (self.cursor.0 / 8 + 1) * 8;
                while self.cursor.0 < next.min(self.cols) {
                    self.put(' ');
                }
            }
            '\u{1B}' => self.escape = 1,
            _ => {}
        }
        self.mark(row);
        let row = self.cursor.1;
        self.mark(row);
    }

    fn char(&mut self, c: char) {
        match self.escape {
            1 => {
                self.escape = if c == '[' { 2 } else { 0 };
                return;
            }
            2 => {
                if ('\u{40}'..='\u{7E}').contains(&c) {
                    self.escape = 0;
                }
                return;
            }
            _ => {}
        }
        if c.is_control() {
            self.control(c);
        } else {
            self.put(c);
        }
    }

    /// Feeds output bytes.
    pub fn write(&mut self, bytes: &[u8]) {
        let mut data = core::mem::take(&mut self.partial);
        data.extend_from_slice(bytes);
        let mut rest = &data[..];
        while !rest.is_empty() {
            match core::str::from_utf8(rest) {
                Ok(s) => {
                    for c in s.chars() {
                        self.char(c);
                    }
                    rest = &[];
                }
                Err(e) => {
                    let (good, bad) = rest.split_at(e.valid_up_to());
                    // Valid by construction.
                    for c in core::str::from_utf8(good).unwrap_or("").chars() {
                        self.char(c);
                    }
                    match e.error_len() {
                        // An unfinished sequence at the end: keep it.
                        None => {
                            self.partial = bad.to_vec();
                            rest = &[];
                        }
                        Some(n) => {
                            self.char('\u{FFFD}');
                            rest = &bad[n..];
                        }
                    }
                }
            }
        }
    }

    /// Marks every row for redrawing.
    pub fn mark_all(&mut self) {
        self.dirty.fill(true);
    }

    /// Takes the rows that changed since the last call.
    pub fn take_dirty(&mut self) -> Vec<usize> {
        let rows = (0..self.rows).filter(|&r| self.dirty[r]).collect();
        self.dirty.fill(false);
        rows
    }

    /// Changes the size, keeping the bottom of the text.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let mut new = Grid::new(cols, rows);
        let keep_from = (self.cursor.1 + 1).saturating_sub(rows);
        for (ny, oy) in (keep_from..self.rows).enumerate().take(rows) {
            for x in 0..cols.min(self.cols) {
                new.cells[ny * cols + x] = self.at(x, oy);
            }
        }
        new.cursor = (self.cursor.0.min(cols - 1), self.cursor.1 - keep_from);
        *self = new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn text(g: &Grid, row: usize) -> String {
        g.row(row).iter().collect::<String>().trim_end().into()
    }

    #[test]
    fn writing_and_wrapping() {
        let mut g = Grid::new(5, 3);
        g.write(b"hello world");
        assert_eq!(text(&g, 0), "hello");
        assert_eq!(text(&g, 1), " worl");
        assert_eq!(text(&g, 2), "d");
        assert_eq!(g.cursor, (1, 2));
        g.write(b"x\ny");
        assert_eq!(text(&g, 0), " worl", "scrolled once");
        assert_eq!(text(&g, 1), "dx");
        assert_eq!(text(&g, 2), "y");
    }

    #[test]
    fn controls() {
        let mut g = Grid::new(20, 2);
        g.write(b"abc\x08 \x08d");
        assert_eq!(text(&g, 0), "abd");
        g.write(b"\rX");
        assert_eq!(text(&g, 0), "Xbd");
        g.write(b"\ta");
        assert_eq!(g.at(8, 0), 'a');
        g.write(b"\x1b[2Jz");
        assert_eq!(g.at(9, 0), 'z');
        g.take_dirty();
        g.write(b"\n");
        assert_eq!(g.take_dirty(), [0, 1]);
        assert!(g.take_dirty().is_empty());
    }

    #[test]
    fn utf8_split_anywhere() {
        let mut g = Grid::new(10, 1);
        let s = "é→x".as_bytes();
        g.write(&s[..1]);
        g.write(&s[1..3]);
        g.write(&s[3..]);
        assert_eq!(text(&g, 0), "é→x");
        g.write(&[0xFF, b'a']);
        assert_eq!(g.at(3, 0), '\u{FFFD}');
        assert_eq!(g.at(4, 0), 'a');
    }

    #[test]
    fn resizing_keeps_the_bottom() {
        let mut g = Grid::new(10, 4);
        g.write(b"1\n2\n3\n4");
        g.resize(5, 2);
        assert_eq!(text(&g, 0), "3");
        assert_eq!(text(&g, 1), "4");
        assert_eq!(g.cursor, (1, 1));
        g.resize(20, 6);
        assert_eq!(text(&g, 1), "4");
    }
}
