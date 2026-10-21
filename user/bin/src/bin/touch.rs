// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! touch: makes each file named if it does not exist (empty).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::sys;
use libstela::fs::open;

libstela::entry!(main);

fn main() -> i64 {
    let args = sys::args();
    if args.is_empty() {
        return sys::fail("touch", "usage: touch file...");
    }
    let mut code = 0;
    for a in &args {
        if let Err(e) = sys::open_path(a, open::READ | open::WRITE | open::CREATE) {
            code = sys::fail("touch", &e);
        }
    }
    code
}
