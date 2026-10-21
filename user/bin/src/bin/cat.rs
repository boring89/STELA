// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! cat: the files named (or the input) one after another.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::sys;

libstela::entry!(main);

fn main() -> i64 {
    match sys::read_input(&sys::args()) {
        Ok(data) => {
            let _ = libstela::console_write(&data);
            0
        }
        Err(e) => sys::fail("cat", &e),
    }
}
