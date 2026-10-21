// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! hexdump: the bytes of the input or the files in hex and as text
//! (`-n N`: the first N bytes only).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::{Args, sys};
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    let args = match Args::parse(&sys::args(), "", &["n"]) {
        Ok(a) => a,
        Err(e) => return sys::fail("hexdump", &e),
    };
    let limit = match args.number("n", u64::MAX) {
        Ok(n) => n,
        Err(e) => return sys::fail("hexdump", &e),
    };
    let data = match sys::read_input(&args.rest) {
        Ok(d) => d,
        Err(e) => return sys::fail("hexdump", &e),
    };
    let data = &data[..(data.len() as u64).min(limit) as usize];
    for (i, chunk) in data.chunks(16).enumerate() {
        println!("{}", cli::hex_line(i as u64 * 16, chunk));
    }
    println!("{:08x}", data.len());
    0
}
