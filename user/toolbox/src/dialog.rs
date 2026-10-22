// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dialogs (P11, the toolbox's Dialog Manager): a modal window of the
//! program's own with its own event loop, which returns the answer.
//!
//! - [`alert`]: a message and buttons (the last one is the default:
//!   Enter); Escape or the close box gives `None`.
//! - [`ask_text`]: a prompt and a text field; OK or Cancel.
//! - [`open_file`] / [`save_file`]: volumes, then folders, then files; a
//!   double click goes into a folder or picks a file; Save takes a name.
//!
//! While a dialog runs, the events of the program's other windows are
//! kept and given back afterwards ([`Connection::put_back`]), so nothing
//! is lost. The layout helpers are pure and tested on the host.

use alloc::string::String;
use alloc::vec::Vec;

/// `text` broken into lines of at most `cols` characters, at spaces
/// where possible; `\n` starts a new line.
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let mut word = word;
            loop {
                let have = line.chars().count();
                let need = word.chars().count() + usize::from(have > 0);
                if have + need <= cols {
                    if have > 0 {
                        line.push(' ');
                    }
                    line.push_str(word);
                    break;
                }
                if have > 0 {
                    out.push(core::mem::take(&mut line));
                    continue;
                }
                // A word longer than a line: cut it.
                let cut = word.char_indices().nth(cols).map_or(word.len(), |(i, _)| i);
                out.push(String::from(&word[..cut]));
                word = &word[cut..];
                if word.is_empty() {
                    break;
                }
            }
        }
        out.push(line);
    }
    out
}

/// What the file list shows for a folder entry and a file.
pub fn entry_label(name: &str, is_dir: bool) -> String {
    if is_dir {
        alloc::format!("{name}/")
    } else {
        String::from(name)
    }
}

/// Folders first, then files, each by name (case-insensitive).
pub fn sort_entries(entries: &mut [(String, bool)]) {
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
}

#[cfg(not(test))]
pub use modal::{alert, ask_text, open_file, save_file};

#[cfg(not(test))]
mod modal {
    use super::*;
    use crate::control::{Button, FieldResult, List, TextField};
    use crate::draw::{Canvas, Font, Rect, color, font_for};
    use crate::event::{Event, MouseKind};
    use crate::window::{Connection, Window};
    use abi::key::{KeyCode, KeyEvent};
    use abi::window::{AUTO, flags};
    use libstela::fs::{self, open};
    use libstela::ipc::{FOREVER, handle_close};
    use libstela::path::Location;

    /// Columns of a dialog's text.
    const COLS: usize = 48;

    /// Runs `step` on the dialog window's events until it answers; the
    /// other windows' events are kept and given back.
    fn run<R>(
        conn: &mut Connection,
        window: &mut Window,
        mut step: impl FnMut(&Event, &mut Window, &Connection) -> Option<Option<R>>,
    ) -> Option<R> {
        let mut kept = Vec::new();
        let answer = loop {
            let Some(event) = conn.next_event(FOREVER) else {
                continue;
            };
            if event.window() != Some(window.id) {
                kept.push(event);
                continue;
            }
            if matches!(event, Event::CloseRequested(_)) {
                break None;
            }
            if let Event::Key { key, .. } = &event
                && key.pressed
                && key.code == KeyCode::Escape
            {
                break None;
            }
            if let Some(answer) = step(&event, window, conn) {
                break answer;
            }
        };
        conn.put_back(kept);
        answer
    }

    fn line(font: &Font) -> i32 {
        font.height + font.height / 4
    }

    /// Buttons right-aligned at the bottom of a `w` x `h` window.
    fn buttons(labels: &[&str], font: &Font, w: i32, h: i32) -> Vec<Button> {
        let bh = font.height + 12;
        let mut x = w - 16;
        let mut out: Vec<Button> = labels
            .iter()
            .rev()
            .map(|l| {
                let bw = (l.chars().count() as i32 + 4) * font.width;
                x -= bw;
                let b = Button::new(Rect::new(x, h - bh - 16, bw, bh), l);
                x -= 12;
                b
            })
            .collect();
        out.reverse();
        out
    }

    fn draw_buttons(c: &mut Canvas, font: &Font, buttons: &[Button]) {
        for (i, b) in buttons.iter().enumerate() {
            b.draw(c, font);
            if i + 1 == buttons.len() {
                // The default button has a heavier frame.
                c.frame(b.rect.inset(-3), color::BLACK);
            }
        }
    }

    /// Mouse handling shared by the dialogs: which button was clicked.
    fn click(buttons: &mut [Button], kind: MouseKind, x: i32, y: i32) -> Option<usize> {
        match kind {
            MouseKind::Down => {
                for b in buttons.iter_mut() {
                    b.press(x, y);
                }
                None
            }
            MouseKind::Up => buttons.iter_mut().position(|b| b.release(x, y)),
            MouseKind::Move => None,
        }
    }

    fn enter(key: &KeyEvent) -> bool {
        key.pressed && key.code == KeyCode::Enter
    }

    /// A message with `buttons`; the index of the one chosen (the last is
    /// the default, Enter), `None` for Escape or the close box.
    pub fn alert(conn: &mut Connection, title: &str, text: &str, labels: &[&str]) -> Option<usize> {
        let font = font_for(conn.screen.0);
        let lh = line(font);
        let lines = wrap(text, COLS);
        let w = (COLS as i32 + 4) * font.width;
        let h = 24 + lines.len() as i32 * lh + 24 + font.height + 12 + 16;
        let mut window = conn
            .create_window(
                w as u32,
                h as u32,
                flags::TITLE_BAR | flags::CLOSE_BOX,
                AUTO,
                AUTO,
                title,
            )
            .ok()?;
        let mut buttons = buttons(labels, font, w, h);
        let draw = |window: &mut Window, buttons: &[Button], conn: &Connection| {
            {
                let mut c = window.canvas();
                c.fill(Rect::new(0, 0, w, h), color::PARCHMENT);
                for (i, l) in lines.iter().enumerate() {
                    c.text(
                        2 * font.width,
                        24 + i as i32 * lh,
                        l,
                        font,
                        color::BLACK,
                        None,
                    );
                }
                draw_buttons(&mut c, font, buttons);
            }
            let _ = conn.damage_all(window);
        };
        draw(&mut window, &buttons, conn);
        let answer = run(conn, &mut window, |event, window, conn| match *event {
            Event::Key { key, .. } if enter(&key) => Some(Some(labels.len().checked_sub(1)?)),
            Event::Mouse { x, y, kind, .. } => {
                let chosen = click(&mut buttons, kind, x, y);
                draw(window, &buttons, conn);
                chosen.map(Some)
            }
            _ => None,
        });
        let _ = conn.close(window);
        answer
    }

    /// `prompt` and a field holding `default`; the text for OK (or
    /// Enter), `None` for Cancel.
    pub fn ask_text(
        conn: &mut Connection,
        title: &str,
        prompt: &str,
        default: &str,
    ) -> Option<String> {
        let font = font_for(conn.screen.0);
        let lh = line(font);
        let lines = wrap(prompt, COLS);
        let w = (COLS as i32 + 4) * font.width;
        let field_y = 24 + lines.len() as i32 * lh + 8;
        let h = field_y + font.height + 12 + 24 + font.height + 12 + 16;
        let mut window = conn
            .create_window(
                w as u32,
                h as u32,
                flags::TITLE_BAR | flags::CLOSE_BOX,
                AUTO,
                AUTO,
                title,
            )
            .ok()?;
        let mut field = TextField::new(
            Rect::new(
                2 * font.width,
                field_y,
                w - 4 * font.width,
                font.height + 12,
            ),
            255,
        );
        field.text = String::from(default);
        field.cursor = default.chars().count();
        let mut buttons = buttons(&["Cancel", "OK"], font, w, h);
        let draw =
            |window: &mut Window, field: &TextField, buttons: &[Button], conn: &Connection| {
                {
                    let mut c = window.canvas();
                    c.fill(Rect::new(0, 0, w, h), color::PARCHMENT);
                    for (i, l) in lines.iter().enumerate() {
                        c.text(
                            2 * font.width,
                            24 + i as i32 * lh,
                            l,
                            font,
                            color::BLACK,
                            None,
                        );
                    }
                    field.draw(&mut c, font, true);
                    draw_buttons(&mut c, font, buttons);
                }
                let _ = conn.damage_all(window);
            };
        draw(&mut window, &field, &buttons, conn);
        let answer = run(conn, &mut window, |event, window, conn| {
            let answer = match *event {
                Event::Key { key, .. } => match field.key(&key) {
                    FieldResult::Enter => Some(Some(field.text.clone())),
                    _ => None,
                },
                Event::Text { ref text, .. } => {
                    field.insert_text(text);
                    None
                }
                Event::Mouse { x, y, kind, .. } => match click(&mut buttons, kind, x, y) {
                    Some(0) => Some(None),
                    Some(_) => Some(Some(field.text.clone())),
                    None => None,
                },
                _ => None,
            };
            draw(window, &field, &buttons, conn);
            answer
        });
        let _ = conn.close(window);
        answer
    }

    /// Where the file dialog is: the volume list, or a folder.
    struct Browser {
        at: Option<Location>,
        /// (name, is a folder) in the list's order (after "../").
        entries: Vec<(String, bool)>,
    }

    impl Browser {
        fn load(&mut self) {
            self.entries.clear();
            match &self.at {
                None => {
                    let mut i = 0;
                    while let Ok(Some(v)) = fs::volume_list(i) {
                        self.entries.push((String::from(v.name()), true));
                        i += 1;
                    }
                }
                Some(loc) => {
                    let Ok(root) = fs::volume_open(&loc.volume) else {
                        return;
                    };
                    let dir = if loc.parts.is_empty() {
                        Ok(root)
                    } else {
                        let d = fs::open(root, &loc.relative(), open::READ);
                        let _ = handle_close(root);
                        d
                    };
                    if let Ok(dir) = dir {
                        for e in fs::entries(dir).unwrap_or_default() {
                            let name = e.name();
                            if !name.starts_with('.') {
                                self.entries
                                    .push((String::from(name), e.kind == abi::kind::DIRECTORY));
                            }
                        }
                        let _ = handle_close(dir);
                    }
                    sort_entries(&mut self.entries);
                }
            }
        }

        fn labels(&self) -> Vec<String> {
            match &self.at {
                None => self
                    .entries
                    .iter()
                    .map(|(n, _)| alloc::format!("{n}:"))
                    .collect(),
                Some(_) => core::iter::once(String::from("../"))
                    .chain(self.entries.iter().map(|(n, d)| entry_label(n, *d)))
                    .collect(),
            }
        }

        /// The entry behind list line `i`: `None` for "../".
        fn entry(&self, i: usize) -> Option<&(String, bool)> {
            match self.at {
                None => self.entries.get(i),
                Some(_) => i.checked_sub(1).and_then(|i| self.entries.get(i)),
            }
        }

        fn title(&self) -> String {
            match &self.at {
                None => String::from("Volumes"),
                Some(loc) => alloc::format!("{loc}"),
            }
        }

        /// Goes into line `i` if it is a folder (or up for "../");
        /// whether it did.
        fn enter(&mut self, i: usize) -> bool {
            let next = match (&self.at, self.entry(i)) {
                (None, Some((name, _))) => Some(Location::root(name)),
                (Some(loc), None) if i == 0 => {
                    if loc.parts.is_empty() {
                        None
                    } else {
                        Some(loc.parent())
                    }
                }
                (Some(loc), Some((name, true))) => {
                    let mut l = loc.clone();
                    l.parts.push(name.clone());
                    Some(l)
                }
                _ => return false,
            };
            self.at = next;
            self.load();
            true
        }
    }

    /// The file dialog: Open (`name` is `None`) or Save (a name field).
    fn file_dialog(
        conn: &mut Connection,
        title: &str,
        start: Option<&Location>,
        name: Option<&str>,
    ) -> Option<Location> {
        let font = font_for(conn.screen.0);
        let lh = line(font);
        let w = 52 * font.width;
        let list_rect = Rect::new(16, 16 + lh, w - 32, 12 * lh);
        let field_y = list_rect.bottom() + 12;
        let saving = name.is_some();
        let h = field_y + if saving { font.height + 24 } else { 0 } + font.height + 12 + 32;
        let mut window = conn
            .create_window(
                w as u32,
                h as u32,
                flags::TITLE_BAR | flags::CLOSE_BOX,
                AUTO,
                AUTO,
                title,
            )
            .ok()?;
        let mut b = Browser {
            at: start.cloned(),
            entries: Vec::new(),
        };
        b.load();
        let mut list = List::new(list_rect, b.labels());
        let mut field = TextField::new(Rect::new(16, field_y, w - 32, font.height + 12), 255);
        if let Some(n) = name {
            field.text = String::from(n);
            field.cursor = n.chars().count();
        }
        let mut buttons = buttons(
            &["Cancel", if saving { "Save" } else { "Open" }],
            font,
            w,
            h,
        );
        let draw = |window: &mut Window,
                    b: &Browser,
                    list: &List,
                    field: &TextField,
                    buttons: &[Button],
                    conn: &Connection| {
            {
                let mut c = window.canvas();
                c.fill(Rect::new(0, 0, w, h), color::PARCHMENT);
                c.text(16, 8, &b.title(), font, color::BLACK, None);
                list.draw(&mut c, font, lh);
                c.frame(list.rect.inset(-1), color::BLACK);
                if saving {
                    field.draw(&mut c, font, true);
                }
                draw_buttons(&mut c, font, buttons);
            }
            let _ = conn.damage_all(window);
        };
        draw(&mut window, &b, &list, &field, &buttons, conn);
        // The answer for line `i` (Open) or the name (Save), going into
        // folders on the way.
        let choose =
            |b: &mut Browser, list: &mut List, field: &TextField| -> Option<Option<Location>> {
                if let Some(i) = list.selected
                    && b.enter(i)
                {
                    list.items = b.labels();
                    list.selected = None;
                    list.top = 0;
                    return None;
                }
                let loc = b.at.clone()?;
                if saving {
                    let name = field.text.trim();
                    if name.is_empty() || name.contains('/') || name.contains(':') {
                        return None;
                    }
                    let mut l = loc;
                    l.parts.push(String::from(name));
                    return Some(Some(l));
                }
                let (name, _) = b.entry(list.selected?)?;
                let mut l = loc;
                l.parts.push(name.clone());
                Some(Some(l))
            };
        let answer = run(conn, &mut window, |event, window, conn| {
            let answer = match *event {
                Event::Key { key, .. } if key.pressed => match key.code {
                    KeyCode::Up | KeyCode::Down => {
                        let n = list.items.len();
                        if n > 0 {
                            let i = match (list.selected, key.code) {
                                (None, _) => 0,
                                (Some(i), KeyCode::Up) => i.saturating_sub(1),
                                (Some(i), _) => (i + 1).min(n - 1),
                            };
                            list.selected = Some(i);
                            let page = list.visible(lh);
                            if i < list.top {
                                list.top = i;
                            } else if i >= list.top + page {
                                list.top = i + 1 - page;
                            }
                        }
                        None
                    }
                    KeyCode::Enter => choose(&mut b, &mut list, &field),
                    _ if saving => {
                        field.key(&key);
                        None
                    }
                    _ => None,
                },
                Event::Text { ref text, .. } if saving => {
                    field.insert_text(text);
                    None
                }
                Event::Mouse {
                    x, y, kind, clicks, ..
                } => {
                    if kind == MouseKind::Down && list.rect.contains(x, y) {
                        let before = list.selected;
                        let hit = list.click(x, y, lh);
                        if clicks >= 2 && hit.is_some() && hit == before {
                            choose(&mut b, &mut list, &field)
                        } else {
                            None
                        }
                    } else {
                        match click(&mut buttons, kind, x, y) {
                            Some(0) => Some(None),
                            Some(_) => choose(&mut b, &mut list, &field),
                            None => None,
                        }
                    }
                }
                _ => None,
            };
            draw(window, &b, &list, &field, &buttons, conn);
            answer
        });
        let _ = conn.close(window);
        answer
    }

    /// Asks for a file to open, starting in `start` (the volumes when
    /// `None`).
    pub fn open_file(
        conn: &mut Connection,
        title: &str,
        start: Option<&Location>,
    ) -> Option<Location> {
        file_dialog(conn, title, start, None)
    }

    /// Asks where to save, starting in `start` with `name` filled in.
    pub fn save_file(
        conn: &mut Connection,
        title: &str,
        start: Option<&Location>,
        name: &str,
    ) -> Option<Location> {
        file_dialog(conn, title, start, Some(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping() {
        assert_eq!(wrap("Delete \"x\"?", 40), ["Delete \"x\"?"]);
        assert_eq!(
            wrap("the quick brown fox jumps", 10),
            ["the quick", "brown fox", "jumps"]
        );
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("abcdefghijk", 4), ["abcd", "efgh", "ijk"]);
        assert_eq!(wrap("", 4), [""]);
    }

    #[test]
    fn entries() {
        let mut e = alloc::vec![
            (String::from("b.txt"), false),
            (String::from("Zeta"), true),
            (String::from("A.txt"), false),
            (String::from("alpha"), true),
        ];
        sort_entries(&mut e);
        let names: Vec<_> = e.iter().map(|(n, d)| entry_label(n, *d)).collect();
        assert_eq!(names, ["alpha/", "Zeta/", "A.txt", "b.txt"]);
    }
}
