// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads address 0, which is never mapped: the page fault ends the process
//! with exit code 128 + 14, and the kernel goes on.

#![no_std]
#![no_main]
#![deny(unsafe_code)]

use libstela::debug_println;

libstela::entry!(main);

fn main() -> i64 {
    debug_println!("fault: reading address 0");
    let value: u8;
    // SAFETY: deliberately faults: this test program exists to check that
    // a page fault in user mode only ends the process. The load is written
    // in assembly because a null dereference in Rust would be undefined
    // behaviour, not a guaranteed fault.
    #[allow(unsafe_code)]
    unsafe {
        core::arch::asm!("mov {}, byte ptr [0]", out(reg_byte) value, options(nostack, readonly))
    };
    debug_println!("fault: read {value}, which should not happen");
    1
}
