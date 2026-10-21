// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! sleep: waits the seconds given (a whole number, or with a fraction:
//! `0.5`).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use cli::sys;

libstela::entry!(main);

/// Nanoseconds in a number of seconds like `2` or `0.25`.
fn ns(text: &str) -> Option<u64> {
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let mut f = 0u64;
    let mut scale = 100_000_000u64;
    for c in frac.chars().take(9) {
        f += u64::from(c.to_digit(10)?) * scale;
        scale /= 10;
    }
    whole.checked_mul(1_000_000_000)?.checked_add(f)
}

fn main() -> i64 {
    let args = sys::args();
    match args.first().and_then(|a| ns(a)) {
        Some(ns) => {
            libstela::sleep_ns(ns);
            0
        }
        None => sys::fail("sleep", "usage: sleep seconds"),
    }
}
