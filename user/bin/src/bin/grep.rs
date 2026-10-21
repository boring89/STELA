// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! grep: the lines that contain a text. `-i` any case, `-n` with line
//! numbers, `-v` the lines that do not. Exit code 0 when something was
//! printed, 1 otherwise (2 for a mistake).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::{Args, sys};
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    let args = match Args::parse(&sys::args(), "inv", &[]) {
        Ok(a) => a,
        Err(e) => return 2 * sys::fail("grep", &e),
    };
    let Some((pattern, files)) = args.rest.split_first() else {
        return 2 * sys::fail("grep", "usage: grep [-inv] text [file...]");
    };
    let text = match sys::read_text(files) {
        Ok(t) => t,
        Err(e) => return 2 * sys::fail("grep", &e),
    };
    let mut found = false;
    for (i, line) in cli::lines(&text).iter().enumerate() {
        if cli::matches(line, pattern, args.has('i')) != args.has('v') {
            found = true;
            if args.has('n') {
                println!("{}:{line}", i + 1);
            } else {
                println!("{line}");
            }
        }
    }
    if found { 0 } else { 1 }
}
