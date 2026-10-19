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
pub mod mm;
#[cfg(target_os = "none")]
pub mod qemu;
#[cfg(target_os = "none")]
pub mod serial;
pub mod sync;
pub mod trap;

/// Brings up what the kernel relies on: serial console, GDT, TSS and IDT
/// (exceptions), the frame allocator, the page tables and the heap. Call
/// once, first thing in the kernel.
#[cfg(target_os = "none")]
pub fn init(info: &'static bootinfo::BootInfo) {
    serial::init();
    arch::x86_64::init();
    mm::init(info);
}

/// Executes `int3`. The breakpoint handler counts it and returns, so this
/// is a harmless way to check that exceptions work. Panics if called before
/// [`init`].
#[cfg(target_os = "none")]
pub fn breakpoint() {
    arch::x86_64::breakpoint();
}

/// Number of breakpoint exceptions handled so far.
#[cfg(target_os = "none")]
pub fn breakpoint_count() -> u64 {
    arch::x86_64::breakpoint_count()
}

/// Prints a panic message to the console. The kernel's `#[panic_handler]`
/// calls this, then halts (or, in a self test, exits QEMU).
#[cfg(target_os = "none")]
pub fn report_panic(info: &core::panic::PanicInfo) {
    kprintln!("\n*** STELA kernel panic ***");
    if let Some(location) = info.location() {
        kprintln!(
            "at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        );
    }
    kprintln!("{}", info.message());
}

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
