// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! wc: counts the lines, words and bytes of its input (`[5]`: a pipe or a
//! file, `docs/userland.md` §6) and prints `wc: L lines, W words, B
//! bytes`. Without an input it says so and ends with 1.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::{io, println};

libstela::entry!(main);

fn main() -> i64 {
    if !io::has_stdin() {
        println!("wc: no input (use it after | or with <)");
        return 1;
    }
    let (mut lines, mut words, mut bytes) = (0u64, 0u64, 0u64);
    let mut in_word = false;
    let mut buf = [0u8; 4096];
    loop {
        let n = match io::read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                println!("wc: {}", e.name());
                return 1;
            }
        };
        bytes += n as u64;
        for &b in &buf[..n] {
            if b == b'\n' {
                lines += 1;
            }
            let space = b.is_ascii_whitespace();
            if !space && !in_word {
                words += 1;
            }
            in_word = !space;
        }
    }
    println!("wc: {lines} lines, {words} words, {bytes} bytes");
    0
}
