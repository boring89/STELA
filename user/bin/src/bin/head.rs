// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! head: the first lines (`-n N`, 10) of the input or the files.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::{Args, sys};
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    let args = match Args::parse(&sys::args(), "", &["n"]) {
        Ok(a) => a,
        Err(e) => return sys::fail("head", &e),
    };
    let n = match args.number("n", 10) {
        Ok(n) => n as usize,
        Err(e) => return sys::fail("head", &e),
    };
    match sys::read_text(&args.rest) {
        Ok(text) => {
            for line in cli::lines(&text).iter().take(n) {
                println!("{line}");
            }
            0
        }
        Err(e) => sys::fail("head", &e),
    }
}
