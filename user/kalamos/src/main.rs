// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Kalamos (`Kalamos.app`, working name; P11, `docs/apps.md` §2): a
//! plain-text editor. One document a window, UTF-8 with `\n` line ends.
//!
//! ```text
//!  Kalamos.app [file]   the file (Volume:/path, or from the working
//!                       folder); a file that does not exist is new
//!  File   New  Open...  Save  Save As...  -  Close      (Ctrl-N O S, Q)
//!  Edit   Undo  -  Cut  Copy  Paste  Select All         (Ctrl-Z X C V A)
//!  View   Small Font  Large Font
//! ```
//!
//! The title is the file's name, with `* ` in front while it has unsaved
//! changes; closing (or New, Open) with unsaved changes asks first. The
//! text is a `toolbox::text::TextView`. It prints what it does on its
//! `[1]` (`kalamos: opened ...`, `saved ... (N bytes)`) for the tests.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::{KeyCode, KeyEvent};
use abi::window::{AUTO, flags};
use alloc::format;
use alloc::string::String;
use cli::sys;
use libstela::fs::open;
use libstela::ipc::FOREVER;
use libstela::path::Location;
use libstela::println;
use toolbox::dialog;
use toolbox::draw::{Font, LARGE, Rect, SMALL, font_for};
use toolbox::event::{Event, MouseKind};
use toolbox::menu::MenuBar;
use toolbox::text::{TextResult, TextView};
use toolbox::window::{Connection, Window};

libstela::entry!(main);

/// Columns and rows of a new window.
const COLS: i32 = 80;
const ROWS: i32 = 30;
/// Largest file opened.
const MAX: usize = 4 << 20;

struct Editor {
    conn: Connection,
    window: Window,
    view: TextView,
    font: &'static Font,
    /// Where the document lives; `None` until it is saved once.
    path: Option<Location>,
    /// The document's file when it came open (P13 K16: handed over at the
    /// start, picked in the powerbox, dropped on the window): saved
    /// through it, not by its path.
    file: Option<sys::Owned>,
    active: bool,
    title: String,
}

/// Menus: (menu, item) as the window server numbers them.
mod item {
    pub const NEW: (u32, u32) = (0, 0);
    pub const OPEN: (u32, u32) = (0, 1);
    pub const SAVE: (u32, u32) = (0, 2);
    pub const SAVE_AS: (u32, u32) = (0, 3);
    pub const CLOSE: (u32, u32) = (0, 5);
    pub const UNDO: (u32, u32) = (1, 0);
    pub const CUT: (u32, u32) = (1, 2);
    pub const COPY: (u32, u32) = (1, 3);
    pub const PASTE: (u32, u32) = (1, 4);
    pub const SELECT_ALL: (u32, u32) = (1, 5);
    pub const SMALL_FONT: (u32, u32) = (2, 0);
    pub const LARGE_FONT: (u32, u32) = (2, 1);
}

impl Editor {
    fn name(&self) -> String {
        self.path
            .as_ref()
            .map_or_else(|| String::from("untitled"), |p| String::from(p.name()))
    }

    /// Draws the text and sets the title.
    fn draw(&mut self) {
        {
            let mut c = self.window.canvas();
            self.view.draw(&mut c, self.font, self.active);
        }
        let _ = self.conn.damage_all(&self.window);
        // Where the IME composes (P13 K3).
        if let Some((x, y, h)) = self.view.caret() {
            let _ = self.conn.set_caret(&self.window, x, y, h);
        }
        let title = if self.view.buffer.is_dirty() {
            format!("* {}", self.name())
        } else {
            self.name()
        };
        if title != self.title {
            let _ = self.conn.set_title(&self.window, &title);
            self.title = title;
        }
    }

    fn bounds(&self) -> Rect {
        self.window.bounds()
    }

    /// Loads `loc` (a missing file is a new, empty document there).
    fn load(&mut self, loc: Location) {
        match sys::open_at(&loc, open::READ) {
            Ok(h) => match sys::read_all(h.0) {
                Ok(data) if data.len() <= MAX => {
                    self.view.buffer.set_text(&String::from_utf8_lossy(&data));
                    println!("kalamos: opened {loc} ({} bytes)", data.len());
                }
                Ok(_) => {
                    println!("kalamos: {loc}: larger than 4 MiB");
                    return;
                }
                Err(e) => {
                    println!("kalamos: {loc}: {}", sys::describe(e));
                    return;
                }
            },
            Err(_) => {
                self.view.buffer.set_text("");
                println!("kalamos: new {loc}");
            }
        }
        self.view.scroll = 0;
        self.path = Some(loc);
        self.file = None;
    }

    /// Loads the open file `file` (P13 K16), which lives at `loc`.
    fn load_file(&mut self, file: sys::Owned, loc: Location) {
        match sys::read_all(file.0) {
            Ok(data) if data.len() <= MAX => {
                self.view.buffer.set_text(&String::from_utf8_lossy(&data));
                println!("kalamos: opened {loc} ({} bytes)", data.len());
                self.view.scroll = 0;
                self.path = Some(loc);
                self.file = Some(file);
            }
            Ok(_) => println!("kalamos: {loc}: larger than 4 MiB"),
            Err(e) => println!("kalamos: {loc}: {}", sys::describe(e)),
        }
    }

    /// Writes the text to the document's file (asking where when it has
    /// none, or with `ask`); whether it was saved.
    fn save(&mut self, ask: bool) -> bool {
        // P13 K16: Save As (and a first save) asks through the toolbox,
        // which uses the powerbox for a program in its container; the
        // file comes open.
        if ask || self.path.is_none() {
            let start = self.path.as_ref().map(Location::parent);
            let name = self.name();
            let Some(p) = dialog::pick_save(&mut self.conn, "Save As", start.as_ref(), &name)
            else {
                return false;
            };
            self.path = Some(p.location);
            self.file = Some(sys::Owned(p.file));
        }
        let Some(loc) = self.path.clone() else {
            return false;
        };
        let text = String::from(self.view.buffer.text());
        let result = match &self.file {
            Some(f) => {
                libstela::fs::set_size(f.0, 0).and_then(|_| sys::write_all(f.0, text.as_bytes(), 0))
            }
            None => sys::open_at(
                &loc,
                open::READ | open::WRITE | open::CREATE | open::TRUNCATE,
            )
            .and_then(|h| sys::write_all(h.0, text.as_bytes(), 0)),
        };
        match result {
            Ok(()) => {
                println!("kalamos: saved {loc} ({} bytes)", text.len());
                self.view.buffer.mark_saved();
                true
            }
            Err(e) => {
                let message = format!("Cannot save {loc}: {}.", sys::describe(e));
                println!("kalamos: {message}");
                let _ = dialog::alert(&mut self.conn, "Kalamos", &message, &["OK"]);
                false
            }
        }
    }

    /// With unsaved changes, asks; whether the document may go.
    fn may_discard(&mut self) -> bool {
        if !self.view.buffer.is_dirty() {
            return true;
        }
        let question = format!("Save the changes to \"{}\"?", self.name());
        match dialog::alert(
            &mut self.conn,
            "Kalamos",
            &question,
            &["Cancel", "Don't Save", "Save"],
        ) {
            Some(1) => true,
            Some(2) => self.save(false),
            _ => false,
        }
    }

    fn copy(&mut self) {
        if let Some(text) = self.view.buffer.selected_text() {
            let text = String::from(text);
            let _ = self.conn.set_clipboard(&text);
        }
    }

    fn set_font(&mut self, font: &'static Font) {
        self.font = font;
        let b = self.bounds();
        self.view.set_layout(b, (font.width, font.height));
    }

    /// A menu item or its key; `false` to end.
    fn command(&mut self, which: (u32, u32)) -> bool {
        match which {
            item::NEW => {
                if self.may_discard() {
                    self.view.buffer.set_text("");
                    self.path = None;
                    self.view.scroll = 0;
                }
            }
            item::OPEN => {
                if self.may_discard() {
                    let start = self.path.as_ref().map(Location::parent);
                    if let Some(p) = dialog::pick_open(&mut self.conn, "Open", start.as_ref()) {
                        self.load_file(sys::Owned(p.file), p.location);
                    }
                }
            }
            item::SAVE => {
                self.save(false);
            }
            item::SAVE_AS => {
                self.save(true);
            }
            item::CLOSE => {
                if self.may_discard() {
                    return false;
                }
            }
            item::UNDO => {
                self.view.buffer.undo();
                self.view.reveal();
            }
            item::CUT => {
                if let Some(text) = self.view.buffer.cut() {
                    let _ = self.conn.set_clipboard(&text);
                }
            }
            item::COPY => self.copy(),
            item::PASTE => {
                if let Ok(text) = self.conn.clipboard()
                    && !text.is_empty()
                {
                    self.view.buffer.insert(&text);
                    self.view.reveal();
                }
            }
            item::SELECT_ALL => self.view.buffer.select_all(),
            item::SMALL_FONT => self.set_font(&SMALL),
            item::LARGE_FONT => self.set_font(&LARGE),
            _ => {}
        }
        true
    }

    /// Ctrl + a letter: the menu item it stands for.
    fn shortcut(key: &KeyEvent) -> Option<(u32, u32)> {
        if !key.pressed || !key.modifiers.ctrl {
            return None;
        }
        Some(match key.code {
            KeyCode::N => item::NEW,
            KeyCode::O => item::OPEN,
            KeyCode::S if key.modifiers.shift => item::SAVE_AS,
            KeyCode::S => item::SAVE,
            KeyCode::Q | KeyCode::W => item::CLOSE,
            KeyCode::Z => item::UNDO,
            KeyCode::X => item::CUT,
            KeyCode::C => item::COPY,
            KeyCode::V => item::PASTE,
            _ => return None,
        })
    }

    /// Handles an event; `false` to end.
    fn event(&mut self, event: Event) -> bool {
        let was_dirty = self.view.buffer.is_dirty();
        let go_on = match event {
            Event::Text { text, .. } => {
                self.view.insert_text(&text);
                true
            }
            Event::Key { key, .. } => match Editor::shortcut(&key) {
                Some(which) => self.command(which),
                None => {
                    self.view.key(&key);
                    true
                }
            },
            Event::Mouse {
                x,
                y,
                kind,
                buttons,
                ..
            } => {
                match kind {
                    MouseKind::Down => {
                        // The window server sends the modifiers with keys
                        // only; Shift-click is not known here.
                        self.view.press(x, y, false);
                    }
                    MouseKind::Move if buttons != 0 => {
                        if self.view.drag(x, y) == TextResult::Ignored {
                            return true;
                        }
                    }
                    MouseKind::Up => self.view.release(),
                    MouseKind::Move => return true,
                }
                true
            }
            Event::Menu(m, i) => self.command((m, i)),
            Event::Focus(_) => {
                self.active = true;
                true
            }
            Event::Blur(_) => {
                self.active = false;
                true
            }
            Event::ResizeRequested(_, w, h) => {
                if self.conn.resize(&mut self.window, w, h).is_ok() {
                    let b = self.bounds();
                    self.view.set_layout(b, (self.font.width, self.font.height));
                }
                true
            }
            Event::CloseRequested(_) => self.command(item::CLOSE),
            // P13 K16: a file dropped from Mouseion.
            Event::Dropped { file, path, .. } => {
                let file = sys::Owned(file);
                if self.may_discard() {
                    match sys::resolve(&path) {
                        Ok(loc) => self.load_file(file, loc),
                        Err(e) => println!("kalamos: {e}"),
                    }
                }
                true
            }
            _ => true,
        };
        if !was_dirty && self.view.buffer.is_dirty() {
            println!("kalamos: modified");
        }
        go_on
    }
}

fn main() -> i64 {
    let args = libstela::process::args();
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("kalamos: no window server: {}", e.name());
            return 1;
        }
    };
    let font = font_for(conn.screen.0);
    let (w, h) = (
        COLS * font.width + 8 + toolbox::text::SCROLL_BAR + 2,
        ROWS * font.height + 2,
    );
    let Ok(window) =
        conn.create_window(w as u32, h as u32, flags::DOCUMENT, AUTO, AUTO, "untitled")
    else {
        return 1;
    };
    let mut menus = MenuBar::new();
    menus.add(
        "File",
        &["New^N", "Open...^O", "Save^S", "Save As...", "-", "Close^W"],
    );
    menus.add(
        "Edit",
        &["Undo^Z", "-", "Cut^X", "Copy^C", "Paste^V", "Select All^A"],
    );
    menus.add("View", &["Small Font", "Large Font"]);
    let _ = conn.set_menubar(&menus.to_text());
    let bounds = window.bounds();
    let mut ed = Editor {
        conn,
        window,
        view: TextView::new(bounds, "", (font.width, font.height)),
        font,
        path: None,
        file: None,
        active: true,
        title: String::from("untitled"),
    };
    // P13 K16: Mouseion hands the file over as [6] (its path is the
    // argument); from the shell the argument is the path.
    let document = libstela::process::document();
    if let Some(name) = args.get(1) {
        match (sys::resolve(name), document) {
            (Ok(loc), Some(doc)) => ed.load_file(sys::Owned(doc), loc),
            (Ok(loc), None) => ed.load(loc),
            (Err(e), _) => println!("kalamos: {e}"),
        }
    }
    ed.draw();
    loop {
        let Some(event) = ed.conn.next_event(FOREVER) else {
            continue;
        };
        if !ed.event(event) {
            break;
        }
        // Draw once the events that came together are handled.
        if !ed.conn.has_pending() {
            ed.draw();
        }
    }
    println!("kalamos: closed");
    let Editor { conn, window, .. } = ed;
    let _ = conn.close(window);
    0
}
