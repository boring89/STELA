// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Multi-line text (P11, the toolbox's TextEdit): [`TextBuffer`] is the
//! text with an insertion point, a selection and one step of undo, laid
//! out in rows of a fixed number of columns (the font is monospaced;
//! long lines wrap); [`TextView`] draws it in a rectangle with a scroll
//! bar and turns keys and the mouse into edits.
//!
//! ```text
//!  text  "one two three\nfour"      cols = 8
//!  rows   0: "one two "   (0..8)
//!         1: "three"      (8..13)    a logical line wraps
//!         2: "four"       (14..18)   after the '\n' at 13
//!  cursor = a byte index; (row, column) comes from the rows
//! ```
//!
//! Columns are cells: a CJK character takes two (`crate::cjk`, P11 K39),
//! everything else one; a tab shows as one blank (the Tab key inserts
//! four spaces).

use crate::control::ScrollBar;
use crate::draw::{Canvas, Font, Rect, color};
use abi::key::{KeyCode, KeyEvent, key_to_char};
use alloc::string::String;
use alloc::vec::Vec;

/// What an edit was, for grouping undo steps: typing letters one after
/// another is one step, and so is deleting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Typing,
    Deleting,
    Other,
}

/// Text with an insertion point and a selection.
#[derive(Clone, Debug, Default)]
pub struct TextBuffer {
    text: String,
    /// The insertion point (a byte index on a character boundary).
    cursor: usize,
    /// The other end of the selection, if there is one.
    anchor: Option<usize>,
    /// The column up and down keep going for.
    goal: Option<usize>,
    /// The text and cursor before the last step, for undo.
    undo: Option<(String, usize)>,
    last: Option<Kind>,
    /// Changed since `mark_saved`.
    dirty: bool,
}

impl TextBuffer {
    /// A buffer holding `text`, the cursor at the start.
    pub fn new(text: &str) -> TextBuffer {
        TextBuffer {
            text: String::from(text),
            ..TextBuffer::default()
        }
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The insertion point (byte index).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the text changed since [`TextBuffer::mark_saved`] (or since
    /// it was made).
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The text was saved.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// The selection as a byte range, if it is not empty.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.cursor).then(|| (a.min(self.cursor), a.max(self.cursor)))
    }

    /// The selected text.
    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(a, b)| &self.text[a..b])
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        self.undo.is_some()
    }

    /// Puts the cursor at `at` (clamped to a boundary); with `extend`, the
    /// selection grows from where it was.
    pub fn set_cursor(&mut self, at: usize, extend: bool) {
        let at = self.boundary(at.min(self.text.len()));
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = at;
        self.last = None;
    }

    /// Selects everything.
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
        self.goal = None;
    }

    fn boundary(&self, mut at: usize) -> usize {
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// Remembers the text for undo unless this edit continues the last
    /// one of the same kind.
    fn begin(&mut self, kind: Kind) {
        if self.last != Some(kind) || kind == Kind::Other {
            self.undo = Some((self.text.clone(), self.cursor));
        }
        self.last = Some(kind);
        self.goal = None;
        self.dirty = true;
    }

    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((a, b)) => {
                self.text.replace_range(a..b, "");
                self.cursor = a;
                self.anchor = None;
                true
            }
            None => {
                self.anchor = None;
                false
            }
        }
    }

    /// Types `s` over the selection (a paste is one undo step; letters
    /// typed one by one are one together).
    pub fn insert(&mut self, s: &str) {
        let kind = if s.chars().count() == 1 && self.selection().is_none() {
            Kind::Typing
        } else {
            Kind::Other
        };
        self.begin(kind);
        self.delete_selection();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    /// Backspace: the selection, or the character before the cursor.
    pub fn backspace(&mut self) {
        if self.selection().is_none() && self.cursor == 0 {
            return;
        }
        self.begin(Kind::Deleting);
        if !self.delete_selection() {
            let before = self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i);
            self.text.replace_range(before..self.cursor, "");
            self.cursor = before;
        }
    }

    /// Delete: the selection, or the character after the cursor.
    pub fn delete_forward(&mut self) {
        if self.selection().is_none() && self.cursor == self.text.len() {
            return;
        }
        self.begin(Kind::Deleting);
        if !self.delete_selection() {
            let after = self.text[self.cursor..]
                .chars()
                .next()
                .map_or(self.cursor, |c| self.cursor + c.len_utf8());
            self.text.replace_range(self.cursor..after, "");
        }
    }

    /// Cut: removes the selection and returns it.
    pub fn cut(&mut self) -> Option<String> {
        let s = String::from(self.selected_text()?);
        self.begin(Kind::Other);
        self.delete_selection();
        Some(s)
    }

    /// Replaces the whole text (a document opened); undo forgets.
    pub fn set_text(&mut self, text: &str) {
        self.text = String::from(text);
        self.cursor = 0;
        self.anchor = None;
        self.goal = None;
        self.undo = None;
        self.last = None;
        self.dirty = false;
    }

    /// Undoes the last step (undo of an undo redoes it).
    pub fn undo(&mut self) -> bool {
        let Some((text, cursor)) = self.undo.take() else {
            return false;
        };
        let now = (core::mem::replace(&mut self.text, text), self.cursor);
        self.cursor = cursor.min(self.text.len());
        self.anchor = None;
        self.undo = Some(now);
        self.last = None;
        self.dirty = true;
        true
    }

    /// The rows of the text laid out `cols` wide: byte ranges, each
    /// without its line's `\n`. There is always at least one row.
    pub fn rows(&self, cols: usize) -> Vec<(usize, usize)> {
        let cols = cols.max(2);
        let mut rows = Vec::new();
        let mut start = 0;
        for line in self.text.split('\n') {
            let mut row = start;
            let mut n = 0;
            for (i, c) in line.char_indices() {
                // A CJK character takes two columns (P11, `crate::cjk`).
                let w = crate::cjk::cells(c);
                if n + w > cols {
                    rows.push((row, start + i));
                    row = start + i;
                    n = 0;
                }
                n += w;
            }
            rows.push((row, start + line.len()));
            start += line.len() + 1;
        }
        rows
    }

    /// The (row, column) of byte `at`.
    pub fn position(&self, rows: &[(usize, usize)], at: usize) -> (usize, usize) {
        // The last row that starts at or before `at`; a position at the
        // end of a full wrapped row belongs to the next row, except at
        // the end of a line.
        let mut row = 0;
        for (i, &(s, e)) in rows.iter().enumerate() {
            if s <= at && (at < e || (at == e && rows.get(i + 1).is_none_or(|n| n.0 > e))) {
                row = i;
                break;
            }
            if s <= at {
                row = i;
            }
        }
        let (s, _) = rows[row];
        (row, crate::cjk::width(&self.text[s..at.max(s)]))
    }

    /// The byte at (row, column), clamped to the row.
    pub fn index_at(&self, rows: &[(usize, usize)], row: usize, col: usize) -> usize {
        let Some(&(s, e)) = rows.get(row.min(rows.len().saturating_sub(1))) else {
            return 0;
        };
        // The character that covers column `col` (the left half of a wide
        // one counts), or the end of the row.
        let mut n = 0;
        for (i, c) in self.text[s..e].char_indices() {
            let w = crate::cjk::cells(c);
            if col < n + w {
                return s + i;
            }
            n += w;
        }
        e
    }

    /// Moves the cursor one character left or right, a row up or down
    /// (keeping the column), or to the start or end of its row; with
    /// `extend` the selection follows. Without it, Left and Right on a
    /// selection go to its start or end.
    pub fn move_cursor(&mut self, key: KeyCode, extend: bool, cols: usize) {
        let rows = self.rows(cols);
        let (row, col) = self.position(&rows, self.cursor);
        if !extend
            && let Some((a, b)) = self.selection()
            && matches!(key, KeyCode::Left | KeyCode::Right)
        {
            self.set_cursor(if key == KeyCode::Left { a } else { b }, false);
            return;
        }
        let at = match key {
            KeyCode::Left => self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i),
            KeyCode::Right => self.text[self.cursor..]
                .chars()
                .next()
                .map_or(self.cursor, |c| self.cursor + c.len_utf8()),
            KeyCode::Up | KeyCode::Down => {
                let goal = self.goal.unwrap_or(col);
                let target = if key == KeyCode::Up {
                    if row == 0 {
                        self.set_cursor(0, extend);
                        self.goal = Some(goal);
                        return;
                    }
                    row - 1
                } else {
                    if row + 1 >= rows.len() {
                        self.set_cursor(self.text.len(), extend);
                        self.goal = Some(goal);
                        return;
                    }
                    row + 1
                };
                let at = self.index_at(&rows, target, goal);
                self.set_cursor(at, extend);
                self.goal = Some(goal);
                return;
            }
            KeyCode::Home => rows[row].0,
            KeyCode::End => rows[row].1,
            _ => return,
        };
        self.set_cursor(at, extend);
    }
}

/// What a key or click did to a [`TextView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextResult {
    /// Nothing for the view.
    Ignored,
    /// The cursor or the selection moved (redraw).
    Moved,
    /// The text changed (redraw; the document is modified).
    Changed,
}

/// Width of the scroll bar.
pub const SCROLL_BAR: i32 = 16;

/// A scrolled, editable [`TextBuffer`] in a rectangle (window
/// coordinates) with a vertical scroll bar on the right.
#[derive(Clone, Debug)]
pub struct TextView {
    /// Where it is (scroll bar included).
    pub rect: Rect,
    /// The text.
    pub buffer: TextBuffer,
    /// The first row shown.
    pub scroll: usize,
    /// Read only: keys move and select but do not edit.
    pub read_only: bool,
    font_size: (i32, i32),
    /// Rows of a mouse selection in progress: the press was here.
    dragging: bool,
}

impl TextView {
    /// A view of `text` in `rect`, laid out for a font of `font_size`
    /// (width, height) pixels.
    pub fn new(rect: Rect, text: &str, font_size: (i32, i32)) -> TextView {
        TextView {
            rect,
            buffer: TextBuffer::new(text),
            scroll: 0,
            read_only: false,
            font_size,
            dragging: false,
        }
    }

    /// The text area (inside the frame, left of the scroll bar).
    pub fn area(&self) -> Rect {
        Rect::new(
            self.rect.x + 1,
            self.rect.y + 1,
            (self.rect.w - SCROLL_BAR - 1).max(0),
            (self.rect.h - 2).max(0),
        )
    }

    /// Columns and rows that fit.
    pub fn size(&self) -> (usize, usize) {
        let a = self.area();
        (
            ((a.w - 8) / self.font_size.0.max(1)).max(1) as usize,
            (a.h / self.font_size.1.max(1)).max(1) as usize,
        )
    }

    /// A new rectangle (the window was resized) or font.
    pub fn set_layout(&mut self, rect: Rect, font_size: (i32, i32)) {
        self.rect = rect;
        self.font_size = font_size;
        self.reveal();
    }

    fn scroll_bar(&self) -> ScrollBar {
        let (cols, page) = self.size();
        ScrollBar {
            rect: Rect::new(
                self.rect.right() - SCROLL_BAR,
                self.rect.y,
                SCROLL_BAR,
                self.rect.h,
            ),
            value: self.scroll,
            total: self.buffer.rows(cols).len(),
            page,
        }
    }

    /// Scrolls so that the cursor shows.
    pub fn reveal(&mut self) {
        let (cols, page) = self.size();
        let rows = self.buffer.rows(cols);
        let (row, _) = self.buffer.position(&rows, self.buffer.cursor());
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + page {
            self.scroll = row + 1 - page;
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(1));
    }

    /// Handles a key press (Ctrl-A selects all; the clipboard keys are the
    /// application's, through its Edit menu).
    pub fn key(&mut self, key: &KeyEvent) -> TextResult {
        if !key.pressed {
            return TextResult::Ignored;
        }
        let (cols, page) = self.size();
        let shift = key.modifiers.shift;
        let result = match key.code {
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End => {
                self.buffer.move_cursor(key.code, shift, cols);
                TextResult::Moved
            }
            KeyCode::PageUp | KeyCode::PageDown => {
                let k = if key.code == KeyCode::PageUp {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                };
                for _ in 0..page.saturating_sub(1).max(1) {
                    self.buffer.move_cursor(k, shift, cols);
                }
                TextResult::Moved
            }
            KeyCode::Delete if !self.read_only => {
                self.buffer.delete_forward();
                TextResult::Changed
            }
            KeyCode::Tab if !self.read_only => {
                self.buffer.insert("    ");
                TextResult::Changed
            }
            _ if key.modifiers.ctrl => match key_to_char(key) {
                Some('\u{1}') => {
                    self.buffer.select_all();
                    TextResult::Moved
                }
                _ => TextResult::Ignored,
            },
            _ if self.read_only => TextResult::Ignored,
            _ => match key_to_char(key) {
                Some('\n' | '\r') => {
                    self.buffer.insert("\n");
                    TextResult::Changed
                }
                Some('\u{8}' | '\u{7F}') => {
                    self.buffer.backspace();
                    TextResult::Changed
                }
                Some(c) if !c.is_control() => {
                    let mut b = [0u8; 4];
                    self.buffer.insert(c.encode_utf8(&mut b));
                    TextResult::Changed
                }
                _ => TextResult::Ignored,
            },
        };
        if result != TextResult::Ignored {
            self.reveal();
        }
        result
    }

    /// The byte under (x, y) (window coordinates; clamped to the text).
    fn hit(&self, x: i32, y: i32) -> usize {
        let a = self.area();
        let (cols, _) = self.size();
        let rows = self.buffer.rows(cols);
        let row = self.scroll as i64 + i64::from((y - a.y).max(0) / self.font_size.1.max(1));
        let col = ((x - a.x - 4).max(0) + self.font_size.0 / 2) / self.font_size.0.max(1);
        let row = (row as usize).min(rows.len() - 1);
        self.buffer.index_at(&rows, row, col as usize)
    }

    /// A press at (x, y): places the cursor (with `shift`, extends the
    /// selection); on the scroll bar, scrolls. Then drags select.
    pub fn press(&mut self, x: i32, y: i32, shift: bool) -> TextResult {
        let mut bar = self.scroll_bar();
        if bar.rect.contains(x, y) {
            if bar.click(x, y) {
                self.scroll = bar.value;
                return TextResult::Moved;
            }
            return TextResult::Ignored;
        }
        if !self.area().contains(x, y) {
            return TextResult::Ignored;
        }
        let at = self.hit(x, y);
        self.buffer.set_cursor(at, shift);
        self.dragging = true;
        TextResult::Moved
    }

    /// The pointer moved with the button down.
    pub fn drag(&mut self, x: i32, y: i32) -> TextResult {
        if !self.dragging {
            return TextResult::Ignored;
        }
        let at = self.hit(x, y);
        self.buffer.set_cursor(at, true);
        self.reveal();
        TextResult::Moved
    }

    /// The button went up.
    pub fn release(&mut self) {
        self.dragging = false;
    }

    /// Scrolls by `rows` (the wheel, or the application).
    pub fn scroll_by(&mut self, rows: i64) {
        let mut bar = self.scroll_bar();
        bar.scroll(rows);
        self.scroll = bar.value;
    }

    /// Draws it: the frame, the visible rows with the selection inverted,
    /// the insertion point (when `active`) and the scroll bar.
    pub fn draw(&self, c: &mut Canvas, font: &Font, active: bool) {
        let a = self.area();
        c.fill(self.rect, color::WHITE);
        c.frame(self.rect, color::BLACK);
        let (cols, page) = self.size();
        let rows = self.buffer.rows(cols);
        let selection = self.buffer.selection();
        let saved = c.clip();
        c.set_clip(saved.intersect(&a));
        let cursor = self.buffer.position(&rows, self.buffer.cursor());
        for (i, &(s, e)) in rows.iter().enumerate().skip(self.scroll).take(page + 1) {
            let y = a.y + (i - self.scroll) as i32 * font.height;
            let mut x = a.x + 4;
            for (off, ch) in self.buffer.text()[s..e].char_indices() {
                let at = s + off;
                let selected = selection.is_some_and(|(p, q)| p <= at && at < q);
                let ch = if ch == '\t' { ' ' } else { ch };
                let cells = if selected {
                    c.char(x, y, ch, font, color::WHITE, Some(color::BLACK))
                } else {
                    c.char(x, y, ch, font, color::BLACK, None)
                };
                x += font.width * cells as i32;
            }
            if active && selection.is_none() && cursor.0 == i {
                let cx = a.x + 4 + cursor.1 as i32 * font.width;
                c.vline(cx, y, font.height, color::BLACK);
            }
        }
        c.set_clip(saved);
        self.scroll_bar().draw(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_wrap_and_split() {
        let b = TextBuffer::new("one two three\nfour\n");
        assert_eq!(b.rows(8), [(0, 8), (8, 13), (14, 18), (19, 19)]);
        assert_eq!(TextBuffer::new("").rows(5), [(0, 0)]);
        // A line exactly as wide as the view is one row.
        assert_eq!(TextBuffer::new("abcd\nx").rows(4), [(0, 4), (5, 6)]);
        // Characters, not bytes.
        assert_eq!(TextBuffer::new("ééé").rows(2), [(0, 4), (4, 6)]);
    }

    #[test]
    fn positions() {
        let b = TextBuffer::new("one two three\nfour");
        let rows = b.rows(8);
        assert_eq!(b.position(&rows, 0), (0, 0));
        // The wrap point belongs to the next row.
        assert_eq!(b.position(&rows, 8), (1, 0));
        // The end of a line stays on its row.
        assert_eq!(b.position(&rows, 13), (1, 5));
        assert_eq!(b.position(&rows, 14), (2, 0));
        assert_eq!(b.position(&rows, 18), (2, 4));
        assert_eq!(b.index_at(&rows, 1, 2), 10);
        assert_eq!(b.index_at(&rows, 2, 99), 18);
        assert_eq!(b.index_at(&rows, 99, 0), 14);
    }

    #[test]
    fn typing_and_undo() {
        let mut b = TextBuffer::new("");
        for c in "hello".chars() {
            b.insert(&String::from(c));
        }
        b.insert(" world");
        assert_eq!(b.text(), "hello world");
        assert!(b.is_dirty());
        // The paste is one step...
        assert!(b.undo());
        assert_eq!(b.text(), "hello");
        // ...and undoing again redoes it.
        assert!(b.undo());
        assert_eq!(b.text(), "hello world");
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "hello wor");
        assert!(b.undo());
        assert_eq!(b.text(), "hello world");
        b.mark_saved();
        assert!(!b.is_dirty());
    }

    #[test]
    fn selections() {
        let mut b = TextBuffer::new("abc def");
        b.set_cursor(4, false);
        b.move_cursor(KeyCode::End, true, 80);
        assert_eq!(b.selected_text(), Some("def"));
        assert_eq!(b.cut().as_deref(), Some("def"));
        assert_eq!(b.text(), "abc ");
        b.set_cursor(0, false);
        b.move_cursor(KeyCode::Right, true, 80);
        b.move_cursor(KeyCode::Right, true, 80);
        b.insert("X");
        assert_eq!(b.text(), "Xc ");
        b.select_all();
        b.delete_forward();
        assert_eq!(b.text(), "");
        // Left on a selection goes to its start.
        let mut b = TextBuffer::new("abcdef");
        b.set_cursor(1, false);
        b.set_cursor(4, true);
        b.move_cursor(KeyCode::Left, false, 80);
        assert_eq!((b.cursor(), b.selection()), (1, None));
    }

    #[test]
    fn up_and_down_keep_the_column() {
        let mut b = TextBuffer::new("long line here\nab\nanother line");
        b.set_cursor(10, false);
        b.move_cursor(KeyCode::Down, false, 80);
        // Row 1 is short: the end of it.
        assert_eq!(b.cursor(), 17);
        b.move_cursor(KeyCode::Down, false, 80);
        // The goal column 10 again.
        assert_eq!(b.cursor(), 18 + 10);
        b.move_cursor(KeyCode::Up, false, 80);
        b.move_cursor(KeyCode::Up, false, 80);
        assert_eq!(b.cursor(), 10);
        b.move_cursor(KeyCode::Up, false, 80);
        assert_eq!(b.cursor(), 0);
        b.move_cursor(KeyCode::Down, true, 80);
        b.move_cursor(KeyCode::Down, true, 80);
        b.move_cursor(KeyCode::Down, true, 80);
        assert_eq!(b.cursor(), b.text().len());
        assert_eq!(b.selection(), Some((0, b.text().len())));
    }

    #[test]
    fn utf8_edits() {
        let mut b = TextBuffer::new("aé");
        b.set_cursor(99, false);
        assert_eq!(b.cursor(), 3);
        b.backspace();
        assert_eq!(b.text(), "a");
        b.insert("€");
        b.move_cursor(KeyCode::Left, false, 80);
        assert_eq!(b.cursor(), 1);
        b.delete_forward();
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn view_keys_and_scrolling() {
        let mut v = TextView::new(Rect::new(0, 0, 100, 50), "", (8, 16));
        // 100 - 16 - 1 - 8 = 75 / 8 = 9 columns; 48 / 16 = 3 rows.
        assert_eq!(v.size(), (9, 3));
        let press = |c: KeyCode, ch: bool| KeyEvent {
            code: c,
            pressed: true,
            modifiers: abi::key::Modifiers {
                shift: ch,
                ..Default::default()
            },
        };
        for _ in 0..5 {
            assert_eq!(v.key(&press(KeyCode::Enter, false)), TextResult::Changed);
        }
        assert_eq!(v.buffer.text(), "\n\n\n\n\n");
        // The cursor is on row 5: rows 3..5 show.
        assert_eq!(v.scroll, 3);
        assert_eq!(v.key(&press(KeyCode::PageUp, false)), TextResult::Moved);
        assert_eq!(v.scroll, 3);
        assert_eq!(v.buffer.position(&v.buffer.rows(9), v.buffer.cursor()).0, 3);
        v.read_only = true;
        assert_eq!(v.key(&press(KeyCode::Enter, false)), TextResult::Ignored);
        assert_eq!(v.key(&press(KeyCode::Down, true)), TextResult::Moved);
        assert!(v.buffer.selection().is_some());
    }

    #[test]
    fn view_mouse() {
        let mut v = TextView::new(Rect::new(0, 0, 200, 100), "hello\nworld", (8, 16));
        // Row 1, column 2: x = 1 + 4 + 2 * 8, y = 1 + 16.
        assert_eq!(v.press(21, 17, false), TextResult::Moved);
        assert_eq!(v.buffer.cursor(), 8);
        assert_eq!(v.drag(5, 1), TextResult::Moved);
        assert_eq!(v.buffer.selected_text(), Some("hello\nwo"));
        v.release();
        assert_eq!(v.drag(50, 50), TextResult::Ignored);
    }

    #[test]
    fn wide_characters() {
        crate::cjk::set_font(Some(crate::cjk::tests::sample()));
        // Each 中 is 3 bytes and two columns.
        let b = TextBuffer::new("中中中x");
        let rows = b.rows(4);
        assert_eq!(rows, [(0, 6), (6, 10)]);
        assert_eq!(b.position(&rows, 3), (0, 2));
        // Column 3 is the right half of the second 中: that character.
        assert_eq!(b.index_at(&rows, 0, 3), 3);
        assert_eq!(b.index_at(&rows, 1, 2), 9);
    }
}
