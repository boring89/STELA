// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! date: the local date and time with the offset from UTC
//! (`Data:/.stela/timezone`, P11 K40; `-u`: UTC).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::sys;
use libstela::{println, process};

libstela::entry!(main);

fn main() -> i64 {
    let utc = sys::args().first().is_some_and(|a| a == "-u");
    match process::wall_clock_ns() {
        Ok(ns) if utc => {
            println!("{} UTC", cli::date(ns));
            0
        }
        Ok(ns) => {
            let offset = libstela::time::offset_minutes();
            let local = ns as i64 + i64::from(offset) * 60_000_000_000;
            println!(
                "{} {}",
                libstela::time::format(local),
                libstela::time::offset_text(offset)
            );
            0
        }
        Err(e) => sys::fail("date", sys::describe(e)),
    }
}
