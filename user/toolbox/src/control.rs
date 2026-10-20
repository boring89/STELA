// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Controls (the toolbox's Control Manager): push buttons, a one-line
//! text field, a vertical scroll bar and a list. Each knows its rectangle
//! (window coordinates), draws itself on a [`Canvas`] and turns mouse and
//! key input into a result; the application decides what that means.

use crate::draw::{Canvas, Font, Rect, color};
use abi::key::{KeyCode, KeyEvent, key_to_char};
use alloc::string::String;
use alloc::vec::Vec;

/// A push button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    /// Where it is.
    pub rect: Rect,
    /// Its label.
    pub label: String,
    /// Held down (the pointer pressed it and is still inside).
    pub pressed: bool,
}

impl Button {
    /// A button.
    pub fn new(rect: Rect, label: &str) -> Button {
        Button {
            rect,
            label: String::from(label),
            pressed: false,
        }
    }

    /// Draws it: a rounded-corner outline, the label centred.
    pub fn draw(&self, c: &mut Canvas, font: &Font) {
        let r = self.rect;
        let (fg, bg) = if self.pressed {
            (color::PARCHMENT, color::BLACK)
        } else {
            (color::BLACK, color::PARCHMENT)
        };
        c.fill(r.inset(1), bg);
        c.hline(r.x + 2, r.y, r.w - 4, color::BLACK);
        c.hline(r.x + 2, r.bottom() - 1, r.w - 4, color::BLACK);
        c.vline(r.x, r.y + 2, r.h - 4, color::BLACK);
        c.vline(r.right() - 1, r.y + 2, r.h - 4, color::BLACK);
        c.pixel(r.x + 1, r.y + 1, color::BLACK);
        c.pixel(r.right() - 2, r.y + 1, color::BLACK);
        c.pixel(r.x + 1, r.bottom() - 2, color::BLACK);
        c.pixel(r.right() - 2, r.bottom() - 2, color::BLACK);
        let tw = font.text_width(&self.label);
        c.text(
            r.x + (r.w - tw) / 2,
            r.y + (r.h - font.height) / 2,
            &self.label,
            font,
            fg,
            None,
        );
    }

    /// A press at (x, y): whether it hit the button.
    pub fn press(&mut self, x: i32, y: i32) -> bool {
        self.pressed = self.rect.contains(x, y);
        self.pressed
    }

    /// The release: whether the button was clicked (pressed and released
    /// inside).
    pub fn release(&mut self, x: i32, y: i32) -> bool {
        let clicked = self.pressed && self.rect.contains(x, y);
        self.pressed = false;
        clicked
    }
}

/// What a key did to a [`TextField`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldResult {
    /// Nothing for the application.
    None,
    /// The text changed.
    Changed,
    /// Enter was pressed.
    Enter,
}

/// A one-line text field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextField {
    /// Where it is.
    pub rect: Rect,
    /// The text.
    pub text: String,
    /// The insertion point, in characters.
    pub cursor: usize,
    /// Longest text, in characters.
    pub max: usize,
}

impl TextField {
    /// An empty field.
    pub fn new(rect: Rect, max: usize) -> TextField {
        TextField {
            rect,
            text: String::new(),
            cursor: 0,
            max,
        }
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Handles a key press.
    pub fn key(&mut self, key: &KeyEvent) -> FieldResult {
        if !key.pressed {
            return FieldResult::None;
        }
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                return FieldResult::None;
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(len);
                return FieldResult::None;
            }
            _ => {}
        }
        match key_to_char(key) {
            Some('\n' | '\r') => FieldResult::Enter,
            Some('\u{8}' | '\u{7F}') => {
                if self.cursor == 0 {
                    return FieldResult::None;
                }
                let at = self.byte_at(self.cursor - 1);
                self.text.remove(at);
                self.cursor -= 1;
                FieldResult::Changed
            }
            Some(c) if !c.is_control() && len < self.max => {
                let at = self.byte_at(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
                FieldResult::Changed
            }
            _ => FieldResult::None,
        }
    }

    /// Draws it with the insertion point (when `active`).
    pub fn draw(&self, c: &mut Canvas, font: &Font, active: bool) {
        let r = self.rect;
        c.fill(r, color::WHITE);
        c.frame(r, color::BLACK);
        let y = r.y + (r.h - font.height) / 2;
        let saved = c.clip();
        c.set_clip(saved.intersect(&r.inset(2)));
        c.text(r.x + 4, y, &self.text, font, color::BLACK, None);
        if active {
            let x = r.x + 4 + self.cursor as i32 * font.width;
            c.vline(x, y, font.height, color::BLACK);
        }
        c.set_clip(saved);
    }
}

/// A vertical scroll bar over `total` lines of which `page` show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollBar {
    /// Where it is.
    pub rect: Rect,
    /// The first line shown.
    pub value: usize,
    /// All lines.
    pub total: usize,
    /// Lines that fit.
    pub page: usize,
}

impl ScrollBar {
    /// The largest `value`.
    pub fn max_value(&self) -> usize {
        self.total.saturating_sub(self.page)
    }

    /// Moves by `delta` lines, kept in range.
    pub fn scroll(&mut self, delta: i64) {
        let v = (self.value as i64 + delta).clamp(0, self.max_value() as i64);
        self.value = v as usize;
    }

    fn arrow(&self) -> i32 {
        self.rect.w
    }

    /// The thumb's rectangle (empty when everything fits).
    pub fn thumb(&self) -> Rect {
        let track = self.rect.h - 2 * self.arrow();
        if self.total <= self.page || track <= 0 {
            return Rect::default();
        }
        let h = ((track as i64 * self.page as i64) / self.total as i64).max(8) as i32;
        let room = track - h;
        let y = self.rect.y
            + self.arrow()
            + (room as i64 * self.value as i64 / self.max_value() as i64) as i32;
        Rect::new(self.rect.x + 1, y, self.rect.w - 2, h)
    }

    /// A click at (x, y): the arrows move a line, the track a page.
    /// Whether the value changed.
    pub fn click(&mut self, x: i32, y: i32) -> bool {
        if !self.rect.contains(x, y) {
            return false;
        }
        let before = self.value;
        let thumb = self.thumb();
        if y < self.rect.y + self.arrow() {
            self.scroll(-1);
        } else if y >= self.rect.bottom() - self.arrow() {
            self.scroll(1);
        } else if y < thumb.y {
            self.scroll(-(self.page as i64));
        } else if y >= thumb.bottom() {
            self.scroll(self.page as i64);
        }
        self.value != before
    }

    /// Draws it: two arrow boxes, the track, the thumb.
    pub fn draw(&self, c: &mut Canvas) {
        let r = self.rect;
        let a = self.arrow();
        c.fill(r, color::STONE);
        c.frame(r, color::BLACK);
        for (top, y) in [(true, r.y), (false, r.bottom() - a)] {
            let b = Rect::new(r.x, y, r.w, a);
            c.fill(b.inset(1), color::PARCHMENT);
            c.frame(b, color::BLACK);
            // A small triangle.
            let cx = r.x + r.w / 2;
            for i in 0..a / 3 {
                let row = if top {
                    y + a / 3 + i
                } else {
                    y + 2 * a / 3 - i
                };
                c.hline(cx - i, row, 2 * i + 1, color::BLACK);
            }
        }
        let t = self.thumb();
        if !t.is_empty() {
            c.fill(t, color::PARCHMENT);
            c.frame(t, color::BLACK);
        }
    }
}

/// A list of lines with one selected, scrolled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    /// Where it is.
    pub rect: Rect,
    /// The lines.
    pub items: Vec<String>,
    /// The selected line.
    pub selected: Option<usize>,
    /// The first line shown.
    pub top: usize,
}

impl List {
    /// A list.
    pub fn new(rect: Rect, items: Vec<String>) -> List {
        List {
            rect,
            items,
            selected: None,
            top: 0,
        }
    }

    /// Lines that fit with a line height of `line`.
    pub fn visible(&self, line: i32) -> usize {
        (self.rect.h / line.max(1)).max(0) as usize
    }

    /// A click at (x, y): the line under it becomes selected.
    pub fn click(&mut self, x: i32, y: i32, line: i32) -> Option<usize> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let i = self.top + ((y - self.rect.y) / line.max(1)) as usize;
        if i < self.items.len() {
            self.selected = Some(i);
            Some(i)
        } else {
            None
        }
    }

    /// Draws the visible lines.
    pub fn draw(&self, c: &mut Canvas, font: &Font, line: i32) {
        let r = self.rect;
        c.fill(r, color::WHITE);
        let saved = c.clip();
        c.set_clip(saved.intersect(&r));
        for (row, i) in (self.top..self.items.len())
            .take(self.visible(line) + 1)
            .enumerate()
        {
            let y = r.y + row as i32 * line;
            let (fg, bg) = if self.selected == Some(i) {
                (color::PARCHMENT, Some(color::HIGHLIGHT))
            } else {
                (color::BLACK, None)
            };
            if let Some(bg) = bg {
                c.fill(Rect::new(r.x, y, r.w, line), bg);
            }
            c.text(
                r.x + 4,
                y + (line - font.height) / 2,
                &self.items[i],
                font,
                fg,
                None,
            );
        }
        c.set_clip(saved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi::key::Modifiers;
    use alloc::vec;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            pressed: true,
            modifiers: Modifiers::from_raw(0),
        }
    }

    #[test]
    fn buttons() {
        let mut b = Button::new(Rect::new(10, 10, 80, 20), "OK");
        assert!(!b.press(0, 0));
        assert!(b.press(20, 15));
        assert!(!b.release(200, 15), "released outside");
        b.press(20, 15);
        assert!(b.release(21, 16));
        let mut px = vec![0u32; 100 * 40];
        let mut c = Canvas::new(&mut px, 100, 40, 100);
        b.draw(&mut c, &crate::draw::SMALL);
        assert_eq!(c.get(12, 10), Some(color::BLACK));
    }

    #[test]
    fn text_fields() {
        let mut f = TextField::new(Rect::new(0, 0, 100, 20), 5);
        assert_eq!(f.key(&press(KeyCode::A)), FieldResult::Changed);
        f.key(&press(KeyCode::B));
        f.key(&press(KeyCode::Left));
        f.key(&press(KeyCode::C));
        assert_eq!(f.text, "acb");
        f.key(&press(KeyCode::Backspace));
        assert_eq!((f.text.as_str(), f.cursor), ("ab", 1));
        for _ in 0..10 {
            f.key(&press(KeyCode::X));
        }
        assert_eq!(f.text.chars().count(), 5);
        assert_eq!(f.key(&press(KeyCode::Enter)), FieldResult::Enter);
        f.cursor = 0;
        assert_eq!(f.key(&press(KeyCode::Backspace)), FieldResult::None);
    }

    #[test]
    fn scroll_bars() {
        let mut s = ScrollBar {
            rect: Rect::new(0, 0, 16, 200),
            value: 0,
            total: 100,
            page: 10,
        };
        assert_eq!(s.max_value(), 90);
        assert!(s.click(8, 190)); // down arrow
        assert_eq!(s.value, 1);
        assert!(s.click(8, 5)); // up arrow
        assert_eq!(s.value, 0);
        assert!(!s.click(8, 5));
        assert!(s.click(8, 150)); // the track below the thumb
        assert_eq!(s.value, 10);
        s.scroll(1000);
        assert_eq!(s.value, 90);
        assert!(!s.thumb().is_empty());
        s.total = 5;
        assert!(s.thumb().is_empty());
    }

    #[test]
    fn lists() {
        let items: Vec<String> = ["a", "b", "c"].iter().map(|s| String::from(*s)).collect();
        let mut l = List::new(Rect::new(0, 0, 50, 40), items);
        assert_eq!(l.visible(20), 2);
        assert_eq!(l.click(5, 25, 20), Some(1));
        assert_eq!(l.selected, Some(1));
        l.top = 1;
        assert_eq!(l.click(5, 25, 20), Some(2));
        assert_eq!(l.click(5, 100, 20), None);
    }
}
