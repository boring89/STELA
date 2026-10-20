// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Viewer (`Viewer.app`): shows a text file, read only, in a scrolling
//! window (`docs/mouseion.md` §6). The file is the first argument
//! (`Volume:/path`, or relative to the working folder); Mouseion opens
//! `.txt` files with it. The arrow and page keys and the scroll bar
//! scroll; File > Close or the close box ends it.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::KeyCode;
use abi::window::{AUTO, flags};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use libstela::fs::{self, open};
use libstela::ipc::{FOREVER, handle_close};
use libstela::path::{self, Location};
use libstela::{Error, bootstrap, println, process};
use toolbox::control::ScrollBar;
use toolbox::draw::{Rect, color, font_for};
use toolbox::event::{Event, MouseKind};
use toolbox::menu::MenuBar;
use toolbox::window::Connection;
use viewer::wrap;

libstela::entry!(main);

const COLS: i32 = 80;
const ROWS: i32 = 30;
const PAD: i32 = 8;
const BAR: i32 = 24;
/// Largest file shown.
const MAX: u64 = 1 << 20;

/// The text of `name` (`Volume:/path` or relative to the working folder).
fn load(name: &str) -> Result<Vec<u8>, Error> {
    let h = if name.contains(':') {
        let loc =
            path::resolve(&Location::root("Data"), name).map_err(|_| Error::InvalidArgument)?;
        let root = fs::volume_open(&loc.volume)?;
        let h = fs::open(root, &loc.relative(), open::READ);
        let _ = handle_close(root);
        h?
    } else {
        fs::open(bootstrap::CWD, name, open::READ)?
    };
    let size = fs::stat(h).map(|s| s.size.min(MAX)).unwrap_or(0);
    let mut data = vec![0u8; size as usize];
    let n = fs::read(h, &mut data, 0).unwrap_or(0);
    data.truncate(n);
    let _ = handle_close(h);
    Ok(data)
}

fn main() -> i64 {
    let args = process::args();
    let Some(name) = args.get(1) else {
        println!("viewer: usage: Viewer file");
        return 2;
    };
    let text = match load(name) {
        Ok(t) => t,
        Err(e) => {
            println!("viewer: {name}: {}", e.name());
            return 1;
        }
    };
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("viewer: no window server: {}", e.name());
            return 1;
        }
    };
    let font = font_for(conn.screen.0);
    let lines = wrap(&text, COLS as usize);
    let title: String = name.rsplit(['/', ':']).next().unwrap_or(name).into();
    let (w, h) = (
        COLS * font.width + 2 * PAD + BAR,
        ROWS * font.height + 2 * PAD,
    );
    let Ok(mut window) =
        conn.create_window(w as u32, h as u32, flags::DOCUMENT, AUTO, AUTO, &title)
    else {
        return 1;
    };
    let mut menus = MenuBar::new();
    menus.add("File", &["Close"]);
    let _ = conn.set_menubar(&menus.to_text());
    println!("viewer: {name}: {} lines", lines.len());
    let mut top = 0usize;
    let mut rows = ROWS as usize;
    loop {
        let bar = ScrollBar {
            rect: Rect::new(
                window.width as i32 - BAR,
                0,
                BAR,
                window.height as i32 - BAR,
            ),
            value: top,
            total: lines.len(),
            page: rows,
        };
        {
            let bounds = window.bounds();
            let mut c = window.canvas();
            c.fill(bounds, color::PARCHMENT);
            for (i, line) in lines.iter().skip(top).take(rows).enumerate() {
                c.text(
                    PAD,
                    PAD + i as i32 * font.height,
                    line,
                    font,
                    color::BLACK,
                    None,
                );
            }
            bar.draw(&mut c);
        }
        let _ = conn.damage_all(&window);
        let mut bar = bar;
        let before = top;
        loop {
            let Some(e) = conn.next_event(FOREVER) else {
                continue;
            };
            match e {
                Event::Key { key, .. } if key.pressed => {
                    let step: i64 = match key.code {
                        KeyCode::Up => -1,
                        KeyCode::Down => 1,
                        KeyCode::PageUp => -(rows as i64),
                        KeyCode::PageDown => rows as i64,
                        KeyCode::Home => -(lines.len() as i64),
                        KeyCode::End => lines.len() as i64,
                        _ => 0,
                    };
                    bar.scroll(step);
                }
                Event::Mouse {
                    x,
                    y,
                    kind: MouseKind::Down,
                    ..
                } => {
                    bar.click(x, y);
                }
                Event::ResizeRequested(_, nw, nh) => {
                    if conn.resize(&mut window, nw, nh).is_ok() {
                        rows = ((nh as i32 - 2 * PAD) / font.height).max(1) as usize;
                        break;
                    }
                }
                Event::Menu(0, 0) | Event::CloseRequested(_) => {
                    let _ = conn.close(window);
                    return 0;
                }
                Event::Other(msg) => {
                    for &h in msg.carried() {
                        let _ = handle_close(h);
                    }
                }
                _ => {}
            }
            if bar.value != before {
                break;
            }
        }
        top = bar.value;
    }
}
