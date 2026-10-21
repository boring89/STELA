// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! date: the date and time (UTC; the RTC is taken as UTC,
//! `docs/time.md` §5.5).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::sys;
use libstela::{println, process};

libstela::entry!(main);

fn main() -> i64 {
    match process::wall_clock_ns() {
        Ok(ns) => {
            println!("{} UTC", cli::date(ns));
            0
        }
        Err(e) => sys::fail("date", sys::describe(e)),
    }
}
