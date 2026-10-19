// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA kernel.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use frame::bootinfo::BootInfo;
use frame::kprintln;

frame::entry_point!(kernel_main);

fn kernel_main(info: &'static BootInfo) -> ! {
    frame::serial::init();
    kprintln!("STELA kernel {}", env!("CARGO_PKG_VERSION"));
    if !info.is_valid() {
        kprintln!(
            "kernel: bad boot information (magic {:#x}, version {})",
            info.magic,
            info.version
        );
        frame::halt();
    }
    let fb = &info.framebuffer;
    kprintln!(
        "kernel: framebuffer {}x{}, pitch {}",
        fb.width,
        fb.height,
        fb.pitch
    );
    frame::halt();
}

#[panic_handler]
fn panic(panic: &core::panic::PanicInfo) -> ! {
    kprintln!("kernel panic: {}", panic);
    frame::halt();
}
