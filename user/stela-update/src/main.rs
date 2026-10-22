// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `stela-update` (`Initrd:/bin/stela-update`, P14 K6, `docs/system.md`
//! §6): the System volume's update.
//!
//! ```text
//! stela-update status                 the slots and their versions
//! stela-update check [--file IMAGE]   what an image (or the package
//!                                     server's index) offers
//! stela-update apply [--file IMAGE]   writes it into the slot that is not
//!                                     mounted; the next start uses it
//! stela-update rollback               the next start uses the other slot
//! ```
//!
//! `apply` and `rollback` need the `POWER` capability: Mouseion's
//! Special, Software Update... starts this program with it (and `--window`: a
//! window shows the progress and offers to restart). From the shell they
//! are refused (the shell has no `POWER`).
//!
//! The image is checked before a byte is written: its trailer's
//! signature (the key of `Initrd:/certs/system.pub`) and the SHA-256 of the
//! rest; the kernel checks the written slot again before it switches.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::KeyCode;
use abi::window::flags;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use libstela::ipc::FOREVER;
use libstela::{Error, fs, println, process};
use toolbox::draw::{color, font_for};
use toolbox::event::Event;
use toolbox::window::{Connection, Window};

libstela::entry!(main);

/// Bytes per `PartitionWrite`.
const CHUNK: usize = 1 << 20;

/// Where the messages go: the console, and the window when there is one.
struct Out {
    gui: Option<(Connection, Window)>,
    lines: Vec<String>,
}

impl Out {
    fn new(window: bool) -> Out {
        let gui = if window {
            Connection::open().ok().and_then(|mut conn| {
                let font = font_for(conn.screen.0);
                let (w, h) = (60 * font.width + 32, 12 * (font.height + 4) + 32);
                let x = (conn.screen.0 as i32 - w) / 2;
                let y = (conn.screen.1 as i32 - h) / 3;
                let win = conn
                    .create_window(w as u32, h as u32, flags::DOCUMENT, x, y, "Software Update")
                    .ok()?;
                Some((conn, win))
            })
        } else {
            None
        };
        Out {
            gui,
            lines: Vec::new(),
        }
    }

    /// One line: printed, and shown in the window.
    fn say(&mut self, text: &str) {
        println!("stela-update: {text}");
        self.lines.push(String::from(text));
        if self.lines.len() > 10 {
            self.lines.remove(0);
        }
        self.draw();
    }

    fn draw(&mut self) {
        let Some((conn, win)) = self.gui.as_mut() else {
            return;
        };
        let font = font_for(conn.screen.0);
        let bounds = win.bounds();
        let mut c = win.canvas();
        c.fill(bounds, color::PARCHMENT);
        for (i, line) in self.lines.iter().enumerate() {
            let y = 16 + i as i32 * (font.height + 4);
            c.text(16, y, line, font, color::BLACK, None);
        }
        let _ = conn.damage_all(win);
    }

    /// In the window: waits for Enter (true) or Esc / the close box.
    fn ask(&mut self, question: &str) -> bool {
        self.say(question);
        let Some((conn, win)) = self.gui.as_mut() else {
            return false;
        };
        let id = win.id;
        loop {
            match conn.next_event(FOREVER) {
                Some(Event::Key { window, key }) if window == id && key.pressed => match key.code {
                    KeyCode::Enter => return true,
                    KeyCode::Escape => return false,
                    _ => {}
                },
                Some(Event::CloseRequested(w)) if w == id => return false,
                _ => {}
            }
        }
    }

    /// Lets the user read the last lines (window only): until a key.
    fn finish(&mut self) {
        if self.gui.is_some() {
            let _ = self.ask("Press Enter to close.");
        }
    }
}

/// The System key (not the package key), the TEST key for now.
const SYSTEM_KEY: &str = "Initrd:/certs/system.pub";

/// The trusted key ([`SYSTEM_KEY`]).
fn key() -> Result<[u8; 32], String> {
    let text = fs::read_path(SYSTEM_KEY).map_err(|e| format!("{SYSTEM_KEY}: {}", e.name()))?;
    sysimage::key_from_hex(&String::from_utf8_lossy(&text))
        .ok_or_else(|| format!("{SYSTEM_KEY}: not a key"))
}

/// Checks a whole image: its trailer is the last block, signed with the
/// key, and the rest hashes to its SHA-256. Returns the version.
fn verify(image: &[u8], name: &str) -> Result<String, String> {
    let n = image.len();
    if n < 2 * sysimage::BLOCK || !n.is_multiple_of(sysimage::BLOCK) {
        return Err(format!("{name}: not a System image (size)"));
    }
    let trailer = &image[n - sysimage::BLOCK..];
    let t = sysimage::parse(trailer).ok_or_else(|| format!("{name}: not a System image"))?;
    if !sysimage::verify(trailer, &key()?) {
        return Err(format!("{name}: the signature is not STELA's"));
    }
    if t.data_len != (n - sysimage::BLOCK) as u64
        || sysimage::sha256(&image[..n - sysimage::BLOCK]) != t.sha256
    {
        return Err(format!("{name}: the image is damaged (SHA-256)"));
    }
    Ok(t.version)
}

/// The image: the file, or the one the package server's index offers.
fn fetch(file: Option<&str>, out: &mut Out) -> Result<(String, Vec<u8>), String> {
    if let Some(f) = file {
        let data = fs::read_path(f).map_err(|e| format!("{f}: {}", e.name()))?;
        return Ok((String::from(f), data));
    }
    if libstela::net::wait_configured(5_000_000_000).is_none() {
        return Err(String::from("the network is not up"));
    }
    let n = pkgcore::ops::update().map_err(|f| f.text())?;
    out.say(&format!("the index lists {n} packages"));
    let Some(r) = pkgcore::ops::system_release().map_err(|f| f.text())? else {
        return Err(String::from("the package server offers no System image"));
    };
    out.say(&format!(
        "the server has System {}; downloading {} KiB",
        r.version,
        r.size / 1024
    ));
    let data = pkgcore::ops::fetch(&r.url).map_err(|f| f.text())?;
    if pkgcore::sha256_hex(&data) != r.sha256 {
        return Err(format!("{}: not the file the index describes", r.url));
    }
    Ok((r.url, data))
}

/// The status line's word `key=`.
fn field<'a>(status: &'a str, key: &str) -> Option<&'a str> {
    status
        .split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn status(out: &mut Out) -> Result<(), String> {
    let s = process::system_status().map_err(|e| String::from(e.name()))?;
    let mounted = field(&s, "mounted").unwrap_or("-");
    out.say(&format!(
        "running slot {mounted}; slot a: {}, slot b: {}",
        field(&s, "a").unwrap_or("-"),
        field(&s, "b").unwrap_or("-")
    ));
    if let Some(rest) = s.split("slot=").nth(1) {
        let file: String = rest.split(" a=").next().unwrap_or("").into();
        out.say(&format!("slot file: {file}"));
    }
    Ok(())
}

fn check(file: Option<&str>, out: &mut Out) -> Result<(), String> {
    let (name, data) = fetch(file, out)?;
    let version = verify(&data, &name)?;
    let running = process::system_status().unwrap_or_default();
    let mounted = field(&running, "mounted").unwrap_or("-");
    let now = match mounted {
        "a" | "b" => field(&running, mounted).unwrap_or("-"),
        _ => "-",
    };
    out.say(&format!(
        "{name}: version {version} (signature and SHA-256 good); running {now}"
    ));
    Ok(())
}

fn apply(file: Option<&str>, out: &mut Out) -> Result<(), String> {
    if process::capability(abi::capability::POWER).is_none() {
        return Err(String::from(
            "no POWER capability: use Mouseion's Special > Software Update...",
        ));
    }
    let (name, data) = fetch(file, out)?;
    let version = verify(&data, &name)?;
    out.say(&format!("{name}: version {version}, signature good"));
    let mut quarter = 0;
    for (i, chunk) in data.chunks(CHUNK).enumerate() {
        process::partition_write((i * CHUNK) as u64, chunk).map_err(|e| match e {
            Error::NotSupported => String::from("this disk has no System slots"),
            Error::NotFound => String::from("there is no other System slot"),
            e => format!("writing the slot: {}", e.name()),
        })?;
        let done = ((i + 1) * CHUNK).min(data.len()) * 4 / data.len();
        while quarter < done {
            quarter += 1;
            out.say(&format!("wrote {}%", quarter * 25));
        }
    }
    process::system_use(true)
        .map_err(|e| format!("the kernel did not take the slot: {}", e.name()))?;
    out.say(&format!("the next start uses version {version}"));
    restart(out);
    Ok(())
}

fn rollback(out: &mut Out) -> Result<(), String> {
    let s = process::system_status().map_err(|e| String::from(e.name()))?;
    let mounted = field(&s, "mounted").unwrap_or("-");
    let file_slot = s
        .split("slot=")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .unwrap_or(mounted);
    // A pending switch is undone; otherwise the other slot.
    let other = file_slot == mounted;
    process::system_use(other).map_err(|e| match e {
        Error::InvalidArgument => String::from("the other slot has no good System image"),
        Error::NotSupported => String::from("this disk has no System slots"),
        e => String::from(e.name()),
    })?;
    let target = if other {
        if mounted == "a" { "b" } else { "a" }
    } else {
        mounted
    };
    out.say(&format!("the next start uses slot {target}"));
    restart(out);
    Ok(())
}

/// In the window: offers to restart now.
fn restart(out: &mut Out) {
    if out.gui.is_none() {
        out.say("restart to use it");
        return;
    }
    if out.ask("Restart now? Enter: restart, Esc: later") {
        out.say("restarting");
        let e = process::reboot();
        out.say(&format!("cannot restart: {}", e.name()));
    }
}

fn main() -> i64 {
    let args = process::args();
    let mut words: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let window = words.contains(&"--window");
    words.retain(|w| *w != "--window");
    let mut file = None;
    if let Some(at) = words.iter().position(|w| *w == "--file") {
        file = words.get(at + 1).copied();
        if file.is_none() {
            println!("stela-update: --file needs a file");
            return 2;
        }
        words.drain(at..at + 2);
    }
    let mut out = Out::new(window);
    let result = match words.as_slice() {
        ["status"] => status(&mut out),
        ["check"] => check(file, &mut out),
        ["apply"] => apply(file, &mut out),
        ["rollback"] => rollback(&mut out),
        _ => {
            println!(
                "usage: stela-update status | check [--file IMAGE] | apply [--file IMAGE] | rollback"
            );
            return 2;
        }
    };
    let code = match result {
        Ok(()) => 0,
        Err(e) => {
            out.say(&e);
            1
        }
    };
    out.finish();
    if out.gui.is_some() {
        println!("stela-update: closed");
    }
    code
}
