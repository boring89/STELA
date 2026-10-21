// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! df: the volumes, their file systems, sizes and free space (the
//! shell's `vols` as a program).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::abi::volume_kind;
use libstela::fs;
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    println!("Volume     Type    Size        Free");
    let mut i = 0;
    while let Ok(Some(v)) = fs::volume_list(i) {
        let kind = match v.kind {
            volume_kind::EHFS => "eHFS",
            volume_kind::FAT => "FAT",
            volume_kind::INITRD => "initrd",
            volume_kind::TMP => "memory",
            _ => "?",
        };
        println!(
            "{:<10} {:<7} {:>8} KiB {:>8} KiB{}",
            v.name(),
            kind,
            v.total / 1024,
            v.free / 1024,
            if v.flags & 1 != 0 { "  read only" } else { "" }
        );
        i += 1;
    }
    0
}
