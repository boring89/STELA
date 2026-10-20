// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Classic.app: a Macintosh IIci in a window (`docs/classic.md`).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    println!("classic: not ready yet");
    1
}
