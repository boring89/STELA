// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Burns the processor for 200 ms without ever yielding, then exits with
//! code 0. Only preemption lets anything else run meanwhile.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::debug_println;

libstela::entry!(main);

/// How long to spin.
const SPIN_NS: u64 = 200_000_000;

fn main() -> i64 {
    let start = libstela::now_ns();
    let mut loops: u64 = 0;
    while libstela::now_ns() - start < SPIN_NS {
        loops = core::hint::black_box(loops + 1);
    }
    debug_println!("spin: {loops} loops in 200 ms");
    0
}
