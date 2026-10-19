// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA framework crate: every `unsafe` operation of the kernel lives
//! here, behind safe APIs. The kernel and drivers are `#![forbid(unsafe_code)]`
//! and reach the hardware only through this crate.
//!
//! Hardware access is compiled only for the bare-metal target
//! (`target_os = "none"`); the pure logic modules also build on the host so
//! that `cargo test` can exercise them.

#![cfg_attr(not(test), no_std)]

#[cfg(target_os = "none")]
pub mod arch;
#[cfg(target_os = "none")]
pub mod console;
#[cfg(target_os = "none")]
pub mod serial;

/// Stops the processor for good: interrupts off, then `hlt` in a loop.
#[cfg(target_os = "none")]
pub fn halt() -> ! {
    arch::x86_64::halt_forever()
}

pub use bootinfo;

/// Defines the kernel's entry symbol `_start`, which the loader jumps to with
/// the address of the [`bootinfo::BootInfo`] in `rdi` (System V AMD64 ABI:
/// first integer argument), and calls `$main` with it.
///
/// The symbol needs `#[unsafe(no_mangle)]`, which the kernel's
/// `#![forbid(unsafe_code)]` does not allow, so it is generated here.
#[macro_export]
macro_rules! entry_point {
    ($main:path) => {
        #[unsafe(no_mangle)]
        extern "C" fn _start(info: &'static $crate::bootinfo::BootInfo) -> ! {
            let main: fn(&'static $crate::bootinfo::BootInfo) -> ! = $main;
            main(info)
        }
    };
}
