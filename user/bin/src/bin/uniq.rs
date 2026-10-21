// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! uniq: repeated adjacent lines once (`-c` with how many).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::{Args, sys};
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    let args = match Args::parse(&sys::args(), "c", &[]) {
        Ok(a) => a,
        Err(e) => return sys::fail("uniq", &e),
    };
    match sys::read_text(&args.rest) {
        Ok(text) => {
            for (n, line) in cli::uniq(&cli::lines(&text)) {
                if args.has('c') {
                    println!("{n:7} {line}");
                } else {
                    println!("{line}");
                }
            }
            0
        }
        Err(e) => sys::fail("uniq", &e),
    }
}
