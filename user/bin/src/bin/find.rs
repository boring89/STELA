// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! find: every item under a folder (default `.`), one path a line;
//! `-name PATTERN` keeps those whose name matches (`*`, `?`).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use cli::{Args, sys};
use libstela::abi::kind;
use libstela::fs::{self, open};
use libstela::path::Location;
use libstela::println;

libstela::entry!(main);

fn walk(dir: &sys::Owned, at: &Location, pattern: Option<&str>, depth: usize) {
    let Ok(entries) = fs::entries(dir.0) else {
        return;
    };
    for e in entries {
        let name = String::from(e.name());
        let mut loc = at.clone();
        loc.parts.push(name.clone());
        if pattern.is_none_or(|p| cli::glob(p, &name)) {
            println!("{loc}");
        }
        if e.kind == kind::DIRECTORY
            && depth < 64
            && let Ok(sub) = fs::open(dir.0, &name, open::READ)
        {
            walk(&sys::Owned(sub), &loc, pattern, depth + 1);
        }
    }
}

fn main() -> i64 {
    let args = match Args::parse(&sys::args(), "", &["name"]) {
        Ok(a) => a,
        Err(e) => return sys::fail("find", &e),
    };
    let start = args.rest.first().map_or(".", String::as_str);
    let (dir, loc) = match sys::open_path(start, open::READ) {
        Ok(d) => d,
        Err(e) => return sys::fail("find", &e),
    };
    walk(&dir, &loc, args.values.get("name").map(String::as_str), 0);
    0
}
