// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! tone: plays a square wave (`tone [Hz [ms]]`, default 440 Hz for 1000
//! ms) through the sound output (`docs/sound.md`).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use libstela::audio::{self, Audio};
use libstela::{println, process};

libstela::entry!(main);

fn main() -> i64 {
    let args = process::args();
    let hz: u32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(440);
    let ms: u32 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(1000);
    let out = match Audio::open() {
        Ok(a) => a,
        Err(e) => {
            println!("tone: no sound output: {}", e.name());
            return 1;
        }
    };
    let total = (abi_rate() as u64 * u64::from(ms) / 1000) as usize;
    println!("tone: {hz} Hz for {ms} ms");
    let mut chunk = alloc::vec![[0i16; 2]; 2048];
    let mut done = 0;
    while done < total {
        let n = chunk.len().min(total - done);
        audio::square(hz, 6000, done, &mut chunk[..n]);
        out.write_all(&chunk[..n]);
        done += n;
    }
    out.drain(5_000_000_000);
    // What the kernel took is still in the card's buffer (~85 ms).
    libstela::sleep_ms(150);
    println!("tone: done ({} underruns)", out.underruns());
    0
}

fn abi_rate() -> u32 {
    libstela::abi::audio::RATE
}
