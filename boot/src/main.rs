// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! STELA's UEFI boot loader. For now it only prints a banner.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use uefi::prelude::*;

#[entry]
fn main() -> Status {
    if let Err(e) = uefi::helpers::init() {
        return e.status();
    }
    uefi::println!("STELA boot loader {}", env!("CARGO_PKG_VERSION"));
    loop {
        uefi::boot::stall(core::time::Duration::from_secs(1));
    }
}
