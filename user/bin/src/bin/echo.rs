// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! echo: prints its arguments, separated by spaces (`-n`: no newline).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;
use alloc::string::String;
use libstela::{print, println};

libstela::entry!(main);

fn main() -> i64 {
    let mut args = cli::sys::args();
    let newline = !(args.first().map(String::as_str) == Some("-n"));
    if !newline {
        args.remove(0);
    }
    let text = args.join(" ");
    if newline {
        println!("{text}");
    } else {
        print!("{text}");
    }
    0
}
