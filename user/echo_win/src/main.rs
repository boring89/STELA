// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! echo_win: the toolbox example (`docs/toolbox.md` §7). A window with a
//! text field, a list of the lines entered and a Clear button; File > Quit
//! (or the close box) ends it. `user/echo` stays for the text mode.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::KeyCode;
use abi::window::flags;
use alloc::vec::Vec;
use libstela::ipc::FOREVER;
use libstela::println;
use toolbox::control::{Button, FieldResult, List, TextField};
use toolbox::draw::{Rect, color, font_for};
use toolbox::event::{Event, MouseKind};
use toolbox::menu::MenuBar;
use toolbox::window::Connection;

libstela::entry!(main);

fn main() -> i64 {
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("echo_win: no window server: {}", e.name());
            return 1;
        }
    };
    let font = font_for(conn.screen.0);
    let (fw, fh) = (font.width, font.height);
    let (w, h) = (40 * fw + 16, 14 * fh);
    // At the right edge of the screen, below the menu bar.
    let (x, y) = (conn.screen.0 as i32 - w - 60, conn.menubar as i32 + 100);
    let Ok(mut window) = conn.create_window(w as u32, h as u32, flags::DOCUMENT, x, y, "Echo")
    else {
        return 1;
    };
    let mut menus = MenuBar::new();
    menus.add("File", &["Clear", "-", "Quit^Q"]);
    menus.add("Edit", &["Copy^C", "Paste^V"]);
    let _ = conn.set_menubar(&menus.to_text());
    let line = fh + 4;
    let mut field = TextField::new(Rect::new(8, 8, w - 16, fh + 8), 38);
    let mut list = List::new(Rect::new(8, fh + 24, w - 16, h - 3 * fh - 40), Vec::new());
    let mut clear = Button::new(
        Rect::new(w - 10 * fw - 8 - fh, h - fh - 16, 10 * fw, fh + 8),
        "Clear",
    );
    let mut dirty = true;
    loop {
        if dirty {
            let bounds = window.bounds();
            let mut c = window.canvas();
            c.fill(bounds, color::PARCHMENT);
            field.draw(&mut c, font, true);
            list.draw(&mut c, font, line);
            c.frame(list.rect, color::BLACK);
            clear.draw(&mut c, font);
            let _ = conn.damage_all(&window);
            dirty = false;
        }
        let Some(event) = conn.next_event(FOREVER) else {
            continue;
        };
        // Edit > Copy (or Ctrl-C): the selected line, else the last one, to
        // the clipboard; Edit > Paste (or Ctrl-V): the clipboard into the
        // field (P11).
        let edit = match &event {
            Event::Key { key, .. } if key.pressed && key.modifiers.ctrl => match key.code {
                KeyCode::C => Some(0),
                KeyCode::V => Some(1),
                _ => None,
            },
            Event::Menu(1, item) => Some(*item),
            _ => None,
        };
        match edit {
            Some(0) => {
                let line = list.selected.or(list.items.len().checked_sub(1));
                if let Some(text) = line.and_then(|i| list.items.get(i))
                    && conn.set_clipboard(text).is_ok()
                {
                    println!("echo_win: copied {text}");
                }
                continue;
            }
            Some(_) => {
                if let Ok(text) = conn.clipboard() {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        if field.text.chars().count() < field.max {
                            field.text.push(c);
                        }
                    }
                    field.cursor = field.text.chars().count();
                    dirty = true;
                }
                continue;
            }
            None => {}
        }
        match event {
            Event::Key { key, .. } => match field.key(&key) {
                FieldResult::Enter if !field.text.is_empty() => {
                    println!("echo_win: {}", field.text);
                    list.items.push(core::mem::take(&mut field.text));
                    field.cursor = 0;
                    let shown = list.visible(line);
                    list.top = list.items.len().saturating_sub(shown);
                    dirty = true;
                }
                FieldResult::Changed => dirty = true,
                _ => {}
            },
            Event::Mouse { x, y, kind, .. } => match kind {
                MouseKind::Down => {
                    dirty |= clear.press(x, y);
                    dirty |= list.click(x, y, line).is_some();
                }
                MouseKind::Up => {
                    if clear.release(x, y) {
                        list.items.clear();
                        list.selected = None;
                    }
                    dirty = true;
                }
                MouseKind::Move => {}
            },
            Event::Menu(0, 0) => {
                list.items.clear();
                dirty = true;
            }
            Event::Menu(0, 2) | Event::CloseRequested(_) => {
                let _ = conn.close(window);
                return 0;
            }
            _ => {}
        }
    }
}
