// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA kernel.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

#[cfg(feature = "selftest")]
mod selftest;

use frame::bootinfo::{self, BootInfo, MemoryKind};
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
    frame::init(info);
    let usable: u64 = info
        .regions()
        .iter()
        .filter(|r| r.kind() == Some(MemoryKind::Usable))
        .map(|r| r.page_count)
        .sum();
    kprintln!(
        "kernel: {} memory regions, {} MiB usable",
        info.regions().len(),
        (usable * bootinfo::PAGE_SIZE) >> 20
    );
    let fb = &info.framebuffer;
    kprintln!(
        "kernel: framebuffer {}x{}, pitch {}",
        fb.width,
        fb.height,
        fb.pitch
    );

    #[cfg(feature = "selftest")]
    selftest::run(info);

    #[cfg(not(feature = "selftest"))]
    {
        kprintln!("kernel: nothing more to do; halting");
        frame::halt();
    }
}

#[panic_handler]
fn panic(panic: &core::panic::PanicInfo) -> ! {
    frame::report_panic(panic);
    #[cfg(feature = "selftest")]
    frame::qemu::exit(frame::qemu::ExitCode::Failure);
    #[cfg(not(feature = "selftest"))]
    frame::halt();
}
