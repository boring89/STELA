// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! wc: counts lines, words and bytes of the input or the files. Without
//! options: `wc: L lines, W words, B bytes`; with `-l`, `-w`, `-c` only
//! those numbers, in that order.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use cli::{Args, sys};
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    let args = match Args::parse(&sys::args(), "lwc", &[]) {
        Ok(a) => a,
        Err(e) => return sys::fail("wc", &e),
    };
    let data = match sys::read_input(&args.rest) {
        Ok(d) => d,
        Err(e) => return sys::fail("wc", &e),
    };
    let (lines, words, bytes) = cli::count(&data);
    if args.flags.is_empty() {
        println!("wc: {lines} lines, {words} words, {bytes} bytes");
        return 0;
    }
    let mut out = Vec::new();
    for (flag, n) in [('l', lines), ('w', words), ('c', bytes)] {
        if args.has(flag) {
            out.push(alloc::format!("{n}"));
        }
    }
    println!("{}", out.join(" "));
    0
}
