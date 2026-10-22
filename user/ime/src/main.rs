// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IME (`Initrd:/ime`, `docs/ime.md`): Zhuyin with libchewing. The
//! dictionary comes from `System:/chewing/` (P14 K4), the phrases it learns go
//! to `Data:/.stela/ime/uhash.dat`.
//!
//! Without arguments it is the service the window server starts (P13 K2):
//! it connects through its bootstrap handle (the server's IME port),
//! registers (`REGISTER_IME`) and answers every `IME_KEY` with an
//! `IME_RESULT`. It starts in English; Ctrl-Space switches. A click on
//! its mark in the menu bar (`IME_SETTINGS`, P14 K1) opens the settings
//! window: the layout and the candidates per page, kept in
//! `Data:/.stela/ime/settings`.
//!
//! `ime type KEYS` types KEYS (ASCII, as on a US keyboard; `\n` is
//! Enter) and prints what is committed: a check from the shell.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::{KeyCode, KeyEvent, Modifiers, key_for_char};
use abi::window::{self as proto, IME_INLINE_MAX, ImeHeader, event, flags, request};
use alloc::string::String;
use alloc::vec::Vec;
use ime::{Composition, DICT_FILES, Files, Ime, Outcome, Settings};
use libstela::fs::open;
use libstela::ipc::{FOREVER, Mapping, handle_close, memory_create};
use libstela::{Error, Message, fs, println, process};
use toolbox::draw::{color, font_for};
use toolbox::event::Event;
use toolbox::window::{Connection, Window};

libstela::entry!(main);

/// Where the dictionary is (P14 K4: `System:/chewing`, else
/// `Initrd:/chewing`).
const DICT_DIR: &str = "chewing";

/// Reads the dictionary files.
fn dictionary() -> Result<Files<'static>, (&'static str, Error)> {
    let mut files = Vec::new();
    for name in DICT_FILES {
        let data = fs::read_system(&alloc::format!("{DICT_DIR}/{name}")).map_err(|e| (name, e))?;
        files.push((name, data));
    }
    Ok(files)
}

/// Starts libchewing (no user phrases: `type` must not change them).
fn start(user: &[u8]) -> Option<Ime> {
    let files = match dictionary() {
        Ok(f) => f,
        Err((name, e)) => {
            println!("ime: System:/{DICT_DIR}/{name}: {}", e.name());
            return None;
        }
    };
    match Ime::new(files, user) {
        Ok(ime) => Some(ime),
        Err(e) => {
            println!("ime: libchewing did not start: {e:?}");
            None
        }
    }
}

/// `ime type KEYS`.
fn type_keys(keys: &str) -> i64 {
    let Some(mut ime) = start(&[]) else {
        return 1;
    };
    let mut text = String::new();
    for c in keys.chars() {
        let event = if c == '\n' {
            Some(KeyEvent {
                code: KeyCode::Enter,
                pressed: true,
                modifiers: Modifiers::default(),
            })
        } else {
            key_for_char(c)
        };
        if let Some(event) = event {
            text.push_str(&ime.key(&event).commit);
        }
    }
    // Whatever is still being composed is committed at the end.
    if ime.composing() {
        text.push_str(
            &ime.key(&KeyEvent {
                code: KeyCode::Enter,
                pressed: true,
                modifiers: Modifiers::default(),
            })
            .commit,
        );
    }
    println!("ime: {text}");
    0
}

/// The user's phrases: `Data:/.stela/ime/uhash.dat`.
const USER_DIR: &str = ".stela/ime";
const USER_FILE: &str = "uhash.dat";

/// The saved user phrases (empty if none).
fn load_user() -> Vec<u8> {
    let mut path = String::from("Data:/");
    path.push_str(USER_DIR);
    path.push('/');
    path.push_str(USER_FILE);
    fs::read_path(&path).unwrap_or_default()
}

/// The settings file in `Data:/.stela/ime/` (P14 K1).
const SETTINGS_FILE: &str = "settings";

/// The saved settings (the defaults if none).
fn load_settings() -> Settings {
    let path = alloc::format!("Data:/{USER_DIR}/{SETTINGS_FILE}");
    fs::read_path(&path)
        .map(|t| Settings::parse(&String::from_utf8_lossy(&t)))
        .unwrap_or_default()
}

/// Saves the user phrases (the folders are made on the way).
fn save_user(data: &[u8]) -> Result<(), Error> {
    save_file(USER_FILE, data)
}

/// Writes `name` in `Data:/.stela/ime/` (the folders are made on the
/// way).
fn save_file(name: &str, data: &[u8]) -> Result<(), Error> {
    let root = fs::volume_open("Data")?;
    let mut dir = root;
    for part in USER_DIR.split('/') {
        let next = fs::open(
            dir,
            part,
            open::READ | open::WRITE | open::CREATE | open::DIRECTORY,
        );
        let _ = handle_close(dir);
        dir = next?;
    }
    let file = fs::open(
        dir,
        name,
        open::READ | open::WRITE | open::CREATE | open::TRUNCATE,
    );
    let _ = handle_close(dir);
    let file = file?;
    let r = fs::write(file, data, 0).map(|_| ());
    let _ = handle_close(file);
    r
}

/// The `IME_RESULT` for a key (`seq`, 0 for none).
fn result(seq: u32, out: &Outcome, c: &Composition) -> Result<Message, Error> {
    let mut list = String::new();
    for (i, cand) in c.candidates.iter().enumerate() {
        if i > 0 {
            list.push('\n');
        }
        list.push_str(cand);
    }
    let mut flags = 0;
    if out.consumed {
        flags |= proto::ime::CONSUMED;
    }
    if c.chinese {
        flags |= proto::ime::CHINESE;
    }
    let mut text = Vec::new();
    text.extend_from_slice(out.commit.as_bytes());
    text.extend_from_slice(c.preedit.as_bytes());
    text.extend_from_slice(list.as_bytes());
    let mut h = ImeHeader {
        seq,
        flags,
        cursor: c.cursor as u32,
        commit_len: out.commit.len() as u32,
        preedit_len: c.preedit.len() as u32,
        candidates_len: list.len() as u32,
        page: c.page as u16,
        pages: c.pages as u16,
    };
    if text.len() <= IME_INLINE_MAX {
        return Ok(proto::ime_result(&h, &text));
    }
    // Too long for the message: the text goes in a Memory.
    h.flags |= proto::ime::MEMORY;
    let size = (text.len() as u64).next_multiple_of(4096);
    let memory = memory_create(size)?;
    let written = Mapping::new(memory, size, true).and_then(|m| m.write(0, &text));
    if let Err(e) = written {
        let _ = handle_close(memory);
        return Err(e);
    }
    let mut msg = proto::ime_result(&h, &[]);
    msg.push_handle(memory);
    Ok(msg)
}

/// The settings window (P14 K1): Up/Down choose a line, Left/Right
/// change it, Enter keeps the settings, Esc closes without them.
struct SettingsWindow {
    window: Window,
    settings: Settings,
    line: usize,
}

impl SettingsWindow {
    fn open(conn: &mut Connection, settings: Settings) -> Option<SettingsWindow> {
        let font = font_for(conn.screen.0);
        let (w, h) = (44 * font.width + 32, 8 * (font.height + 4) + 32);
        let x = (conn.screen.0 as i32 - w) / 2;
        let y = (conn.screen.1 as i32 - h) / 3;
        let window = conn
            .create_window(w as u32, h as u32, flags::DOCUMENT, x, y, "注音輸入法")
            .ok()?;
        let mut s = SettingsWindow {
            window,
            settings,
            line: 0,
        };
        s.draw(conn);
        println!("ime: settings open");
        Some(s)
    }

    fn draw(&mut self, conn: &mut Connection) {
        let font = font_for(conn.screen.0);
        let line_h = font.height + 4;
        let bounds = self.window.bounds();
        let mut c = self.window.canvas();
        c.fill(bounds, color::PARCHMENT);
        let mut layouts = String::new();
        for l in ime::Layout::ALL {
            if l == self.settings.layout {
                layouts.push_str(&alloc::format!("[{}] ", l.label()));
            } else {
                layouts.push_str(&alloc::format!(" {}  ", l.label()));
            }
        }
        let lines = [
            alloc::format!("鍵盤配置 Layout:  {layouts}"),
            alloc::format!("候選字數 Candidates:  < {} >", self.settings.candidates),
        ];
        let mut y = 16;
        for (i, text) in lines.iter().enumerate() {
            let mark = if i == self.line { ">" } else { " " };
            c.text(16, y, mark, font, color::BLACK, None);
            c.text(16 + 2 * font.width, y, text, font, color::BLACK, None);
            y += 2 * line_h;
        }
        c.text(
            16,
            bounds.h - line_h - 16,
            "Enter: OK   Esc: Cancel   ←→: change",
            font,
            color::BLACK,
            None,
        );
        let _ = conn.damage_all(&self.window);
    }

    /// A key for the window: `Some(true)` keep the settings, `Some(false)`
    /// close without them.
    fn key(&mut self, conn: &mut Connection, key: &KeyEvent) -> Option<bool> {
        if !key.pressed {
            return None;
        }
        match key.code {
            KeyCode::Enter => return Some(true),
            KeyCode::Escape => return Some(false),
            KeyCode::Up => self.line = 0,
            KeyCode::Down => self.line = 1,
            KeyCode::Left | KeyCode::Right => {
                let forward = key.code == KeyCode::Right;
                if self.line == 0 {
                    self.settings.step_layout(forward);
                } else {
                    self.settings.step_candidates(forward);
                }
            }
            _ => return None,
        }
        self.draw(conn);
        None
    }
}

/// The service.
fn serve() -> i64 {
    let user = load_user();
    let Some(mut ime) = start(&user) else {
        return 1;
    };
    let mut settings = load_settings();
    ime.apply(&settings);
    ime.set_chinese(false);
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("ime: no window server: {}", e.name());
            return 1;
        }
    };
    if let Err(e) = conn.send(&proto::register_ime()) {
        println!("ime: cannot register: {}", e.name());
        return 1;
    }
    println!(
        "ime: ready (libchewing 0.5.1, {}, {} candidates)",
        settings.layout.code(),
        settings.candidates
    );
    // `make test-ime` (P14 K1): the boot option imecrash ends the IME
    // right away, to check that the window server starts it again once.
    if process::boot_flag("imecrash") {
        println!("ime: imecrash: ending");
        return 1;
    }
    let mut panel: Option<SettingsWindow> = None;
    // What the log shows (for the tests; never the text itself).
    let mut chinese = false;
    let mut listing = false;
    loop {
        let Some(ev) = conn.next_event(FOREVER) else {
            continue;
        };
        let (seq, out) = match ev {
            Event::Other(msg) if msg.tag == event::IME_KEY => {
                let Some((seq, key)) = proto::ime_key_of(&msg) else {
                    continue;
                };
                (seq, ime.key(&key))
            }
            Event::Other(msg) if msg.tag == event::IME_RESET => {
                ime.reset();
                (0, Outcome::default())
            }
            Event::Other(msg) if msg.tag == event::IME_SETTINGS => {
                match &panel {
                    Some(p) => {
                        let _ = conn.focus(&p.window);
                    }
                    None => panel = SettingsWindow::open(&mut conn, settings),
                }
                continue;
            }
            Event::Key { window, key } if panel.as_ref().is_some_and(|p| p.window.id == window) => {
                let done = panel.as_mut().and_then(|p| p.key(&mut conn, &key));
                if let Some(keep) = done
                    && let Some(p) = panel.take()
                {
                    if keep {
                        settings = p.settings;
                        ime.apply(&settings);
                        match save_file(SETTINGS_FILE, settings.text().as_bytes()) {
                            Ok(()) => println!(
                                "ime: settings {}, {} candidates",
                                settings.layout.code(),
                                settings.candidates
                            ),
                            Err(e) => println!("ime: cannot save the settings: {}", e.name()),
                        }
                    }
                    let _ = conn.close(p.window);
                }
                continue;
            }
            Event::CloseRequested(window)
                if panel.as_ref().is_some_and(|p| p.window.id == window) =>
            {
                if let Some(p) = panel.take() {
                    let _ = conn.close(p.window);
                }
                continue;
            }
            Event::Error(req, e) if req == request::REGISTER_IME => {
                println!(
                    "ime: the window server refused: {}",
                    Error::from_raw(u64::from(e)).map_or("?", Error::name)
                );
                return 1;
            }
            _ => continue,
        };
        let comp = ime.composition();
        if comp.chinese != chinese {
            chinese = comp.chinese;
            println!("ime: {}", if chinese { "注" } else { "英" });
        }
        if comp.candidates.is_empty() == listing {
            listing = !listing;
            if listing {
                println!("ime: candidates");
            }
        }
        match result(seq, &out, &comp) {
            Ok(msg) => {
                if conn.send(&msg).is_err() {
                    return 0;
                }
            }
            Err(e) => println!("ime: cannot answer: {}", e.name()),
        }
        if let Some(data) = ime.changed_user_data()
            && let Err(e) = save_user(&data)
        {
            println!("ime: cannot save the user phrases: {}", e.name());
        }
    }
}

fn main() -> i64 {
    let args = process::args();
    match args.get(1).map(String::as_str) {
        None => serve(),
        Some("type") => type_keys(args.get(2).map_or("", String::as_str)),
        _ => {
            println!("ime: usage: ime [type KEYS]");
            2
        }
    }
}
