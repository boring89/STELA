// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The IME (`Initrd:/ime`, `docs/ime.md`): Zhuyin with libchewing. The
//! dictionary comes from `Initrd:/chewing/`, the phrases it learns go
//! to `Data:/.stela/ime/uhash.dat`.
//!
//! `ime type KEYS` types KEYS (ASCII, as on a US keyboard; `\n` is
//! Enter) and prints what is committed: a check from the shell.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::key::{KeyCode, KeyEvent, Modifiers, key_for_char};
use alloc::string::String;
use alloc::vec::Vec;
use ime::{DICT_FILES, Files, Ime};
use libstela::{Error, fs, println, process};

libstela::entry!(main);

/// Where the dictionary is.
const DICT_DIR: &str = "Initrd:/chewing";

/// Reads the dictionary files.
fn dictionary() -> Result<Files<'static>, (&'static str, Error)> {
    let mut files = Vec::new();
    for name in DICT_FILES {
        let mut path = String::from(DICT_DIR);
        path.push('/');
        path.push_str(name);
        let data = fs::read_path(&path).map_err(|e| (name, e))?;
        files.push((name, data));
    }
    Ok(files)
}

/// Starts libchewing (no user phrases: `type` must not change them).
fn start(user: &[u8]) -> Option<Ime> {
    let files = match dictionary() {
        Ok(f) => f,
        Err((name, e)) => {
            println!("ime: {DICT_DIR}/{name}: {}", e.name());
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

fn main() -> i64 {
    let args = process::args();
    match args.get(1).map(String::as_str) {
        Some("type") => type_keys(args.get(2).map_or("", String::as_str)),
        _ => {
            println!("ime: usage: ime type KEYS");
            2
        }
    }
}
