// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! false: does nothing, with exit code 1.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

libstela::entry!(main);

fn main() -> i64 {
    1
}
