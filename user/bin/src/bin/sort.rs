// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! sort: the lines in order (`-n` by their leading number, `-r`
//! reversed).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::{Args, sys};
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    let args = match Args::parse(&sys::args(), "nr", &[]) {
        Ok(a) => a,
        Err(e) => return sys::fail("sort", &e),
    };
    match sys::read_text(&args.rest) {
        Ok(text) => {
            let mut lines = cli::lines(&text);
            cli::sort(&mut lines, args.has('n'), args.has('r'));
            for line in lines {
                println!("{line}");
            }
            0
        }
        Err(e) => sys::fail("sort", &e),
    }
}
