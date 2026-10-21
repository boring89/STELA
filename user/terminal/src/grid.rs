// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A grid of character cells fed with a program's output (UTF-8 bytes,
//! possibly split anywhere): printable characters advance the cursor and
//! wrap; `\n` goes to the next line (scrolling at the bottom), `\r` to the
//! start of the line, backspace one cell back, tab to the next multiple
//! of 8. Of the escape sequences (`ESC [ ... final`), SGR (`m`) sets the
//! colours (P11: 30-37, 40-47, 90-97, 100-107, 39, 49, 1 bold = bright,
//! 0 reset); the others are skipped. Lines that scroll off the top are
//! kept (1000, `SCROLLBACK`) and can be looked at again (`scroll_by`).
//! Rows that changed are remembered for redrawing. Pure logic, tested on
//! the host.

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

/// Lines kept above the screen.
pub const SCROLLBACK: usize = 1000;
/// "The terminal's own colour" (not one of the 16).
pub const DEFAULT: u8 = 0xFF;

/// One character cell: the character and its colours (0-15 of the
/// ANSI palette, or `DEFAULT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The character.
    pub c: char,
    /// Foreground colour.
    pub fg: u8,
    /// Background colour.
    pub bg: u8,
}

impl Cell {
    /// A blank in the default colours.
    pub const BLANK: Cell = Cell {
        c: ' ',
        fg: DEFAULT,
        bg: DEFAULT,
    };
}

/// The grid.
#[derive(Debug)]
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    /// The cursor's column and row.
    pub cursor: (usize, usize),
    dirty: Vec<bool>,
    /// Bytes of an unfinished UTF-8 sequence.
    partial: Vec<u8>,
    /// Inside an escape sequence: 1 after ESC, 2 after `ESC [`.
    escape: u8,
    /// The parameters of the sequence so far.
    params: Vec<u8>,
    /// The colours new characters get, and whether bold makes the
    /// foreground bright.
    fg: u8,
    bg: u8,
    bold: bool,
    /// Lines that scrolled off the top, oldest first.
    history: VecDeque<Vec<Cell>>,
    /// How many lines the view is scrolled back (0: the live screen).
    scroll: usize,
}

impl Grid {
    /// An empty grid of `cols` x `rows` (at least 1 x 1).
    pub fn new(cols: usize, rows: usize) -> Grid {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Grid {
            cols,
            rows,
            cells: vec![Cell::BLANK; cols * rows],
            cursor: (0, 0),
            dirty: vec![true; rows],
            partial: Vec::new(),
            escape: 0,
            params: Vec::new(),
            fg: DEFAULT,
            bg: DEFAULT,
            bold: false,
            history: VecDeque::new(),
            scroll: 0,
        }
    }

    /// One row of the view (the history while scrolled back).
    pub fn view_row(&self, row: usize) -> &[Cell] {
        if row < self.scroll {
            let h = self.history.len() - self.scroll + row;
            &self.history[h]
        } else {
            let r = row - self.scroll;
            &self.cells[r * self.cols..(r + 1) * self.cols]
        }
    }

    /// Scrolls the view back (`lines` > 0) or forward, within the
    /// history; returns whether it moved.
    pub fn scroll_by(&mut self, lines: isize) -> bool {
        let max = self.history.len().min(SCROLLBACK) as isize;
        let new = (self.scroll as isize + lines).clamp(0, max) as usize;
        if new == self.scroll {
            return false;
        }
        self.scroll = new;
        self.dirty.fill(true);
        true
    }

    /// How far the view is scrolled back.
    pub fn scrolled(&self) -> usize {
        self.scroll
    }

    /// Lines kept above the screen.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Columns and rows.
    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// The character at (col, row) of the live screen.
    pub fn at(&self, col: usize, row: usize) -> char {
        self.cells[row * self.cols + col].c
    }

    /// The cell at (col, row) of the live screen.
    pub fn cell(&self, col: usize, row: usize) -> Cell {
        self.cells[row * self.cols + col]
    }

    /// One row of the live screen.
    pub fn row(&self, row: usize) -> &[Cell] {
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
            // The top line goes into the history.
            self.history.push_back(self.cells[..self.cols].to_vec());
            if self.history.len() > SCROLLBACK {
                self.history.pop_front();
            }
            self.cells.copy_within(self.cols.., 0);
            let last = (self.rows - 1) * self.cols;
            let blank = self.blank();
            self.cells[last..].fill(blank);
            self.dirty.fill(true);
        }
    }

    /// A blank in the current background.
    fn blank(&self) -> Cell {
        Cell {
            c: ' ',
            fg: DEFAULT,
            bg: self.bg,
        }
    }

    /// The foreground new characters get.
    fn foreground(&self) -> u8 {
        if self.bold && self.fg < 8 {
            self.fg + 8
        } else {
            self.fg
        }
    }

    /// Select Graphic Rendition (ECMA-48 8.3.117) with the parameters.
    fn sgr(&mut self) {
        let text = core::str::from_utf8(&self.params).unwrap_or("");
        let mut any = false;
        for p in text.split(';') {
            any = true;
            match p.parse::<u8>().unwrap_or(0) {
                0 => {
                    self.fg = DEFAULT;
                    self.bg = DEFAULT;
                    self.bold = false;
                }
                1 => self.bold = true,
                22 => self.bold = false,
                n @ 30..=37 => self.fg = n - 30,
                39 => self.fg = DEFAULT,
                n @ 40..=47 => self.bg = n - 40,
                49 => self.bg = DEFAULT,
                n @ 90..=97 => self.fg = n - 90 + 8,
                n @ 100..=107 => self.bg = n - 100 + 8,
                _ => {}
            }
        }
        if !any {
            self.fg = DEFAULT;
            self.bg = DEFAULT;
            self.bold = false;
        }
    }

    fn put(&mut self, c: char) {
        if self.cursor.0 >= self.cols {
            self.newline();
        }
        let (x, y) = self.cursor;
        self.cells[y * self.cols + x] = Cell {
            c,
            fg: self.foreground(),
            bg: self.bg,
        };
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
            '\u{1B}' => {
                self.escape = 1;
                self.params.clear();
            }
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
                    if c == 'm' {
                        self.sgr();
                    }
                } else if self.params.len() < 32 && c.is_ascii() {
                    self.params.push(c as u8);
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

    /// Feeds output bytes (the view jumps back to the live screen).
    pub fn write(&mut self, bytes: &[u8]) {
        if self.scroll != 0 {
            self.scroll = 0;
            self.dirty.fill(true);
        }
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
                new.cells[ny * cols + x] = self.cell(x, oy);
            }
        }
        new.cursor = (self.cursor.0.min(cols - 1), self.cursor.1 - keep_from);
        // The history stays (lines are cut or padded to the new width).
        let pushed: Vec<Vec<Cell>> = (0..keep_from).map(|y| self.row(y).to_vec()).collect();
        new.history = core::mem::take(&mut self.history)
            .into_iter()
            .chain(pushed)
            .map(|mut line| {
                line.resize(cols, Cell::BLANK);
                line
            })
            .collect();
        while new.history.len() > SCROLLBACK {
            new.history.pop_front();
        }
        (new.fg, new.bg, new.bold) = (self.fg, self.bg, self.bold);
        *self = new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn text(g: &Grid, row: usize) -> String {
        g.row(row)
            .iter()
            .map(|c| c.c)
            .collect::<String>()
            .trim_end()
            .into()
    }

    fn view(g: &Grid, row: usize) -> String {
        g.view_row(row)
            .iter()
            .map(|c| c.c)
            .collect::<String>()
            .trim_end()
            .into()
    }

    #[test]
    fn colours() {
        let mut g = Grid::new(20, 2);
        g.write(b"a\x1b[31mb\x1b[1;42mc\x1b[0md\x1b[94me\x1b[mf");
        let fg: Vec<u8> = (0..6).map(|x| g.cell(x, 0).fg).collect();
        assert_eq!(fg, [DEFAULT, 1, 9, DEFAULT, 12, DEFAULT]);
        assert_eq!(g.cell(2, 0).bg, 2);
        assert_eq!(g.cell(3, 0).bg, DEFAULT);
        assert_eq!(text(&g, 0), "abcdef");
        // Other sequences are skipped.
        g.write(b"\x1b[2J\x1b[10;20Hg");
        assert_eq!(g.at(6, 0), 'g');
    }

    #[test]
    fn scrolling_back() {
        let mut g = Grid::new(10, 3);
        for i in 0..10 {
            g.write(alloc::format!("line {i}\n").as_bytes());
        }
        assert_eq!(g.history_len(), 8);
        assert_eq!(text(&g, 0), "line 8");
        assert!(g.scroll_by(2));
        assert_eq!(view(&g, 0), "line 6");
        assert_eq!(view(&g, 2), "line 8");
        assert!(g.scroll_by(100));
        assert_eq!(g.scrolled(), 8);
        assert_eq!(view(&g, 0), "line 0");
        assert!(!g.scroll_by(1));
        // New output jumps back to the live screen.
        g.write(b"x");
        assert_eq!(g.scrolled(), 0);
        assert_eq!(view(&g, 2), "x");
        for i in 0..2000 {
            g.write(alloc::format!("{i}\n").as_bytes());
        }
        assert_eq!(g.history_len(), SCROLLBACK);
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
