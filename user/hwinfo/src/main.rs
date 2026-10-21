// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! hwinfo: prints the hardware report (`SysInfo(HARDWARE)`,
//! `docs/hwinfo.md`): the processor, memory, display, the memory map,
//! ACPI tables, PCI functions, USB devices, disks and partitions,
//! volumes, the network card and the sound card. `hwinfo > file` keeps it.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::{print, println, process};

libstela::entry!(main);

fn main() -> i64 {
    match process::hardware() {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(e) => {
            println!("hwinfo: {}", e.name());
            1
        }
    }
}
