// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA kernel.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(feature = "selftest")]
mod selftest;

use frame::bootinfo::{self, BootInfo, MemoryKind};
use frame::kprintln;

frame::entry_point!(kernel_main);

fn kernel_main(info: &'static BootInfo) -> ! {
    frame::serial::init();
    if !info.is_valid() {
        kprintln!(
            "kernel: bad boot information (magic {:#x}, version {})",
            info.magic,
            info.version
        );
        frame::halt();
    }
    frame::init(info);
    kernel::task::init();
    kernel::syscall::init();
    kprintln!("STELA kernel {}", env!("CARGO_PKG_VERSION"));
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
    let (free, _) = frame::mm::pmm::stats();
    kprintln!(
        "kernel: {} MiB free after setup, heap {} KiB",
        (free as u64 * bootinfo::PAGE_SIZE) >> 20,
        frame::mm::heap::stats().size / 1024
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
        if let Err(e) = kernel::task::spawn("init", init) {
            panic!("kernel: cannot start the init thread: {e:?}");
        }
        // The boot thread's work is done; the scheduler takes over.
        kernel::task::exit();
    }
}

/// The first kernel thread: runs `hello` from the initrd, waits for it,
/// then becomes the echo loop.
#[cfg(not(feature = "selftest"))]
fn init() {
    match kernel::program::spawn_from_initrd("hello") {
        Ok(pid) => {
            let code = kernel::task::wait_process(pid).unwrap_or(-1);
            kprintln!("kernel: hello (process {pid}) exited with code {code}");
        }
        Err(e) => kprintln!("kernel: cannot start hello from the initrd: {e:?}"),
    }
    echo_loop();
}

/// Echoes what is typed and prints the uptime on Enter, sleeping between
/// events. A kernel thread; a placeholder until there is a shell (P6).
#[cfg(not(feature = "selftest"))]
fn echo_loop() {
    use frame::input::{self, InputEvent};
    kprintln!("kernel: ready; type to echo, Enter prints the uptime");
    loop {
        let Some(InputEvent::Key(event)) = input::pop() else {
            // Poll the input queue every few milliseconds.
            kernel::task::sleep_ms(4);
            continue;
        };
        match input::key_to_char(&event) {
            Some('\n') => {
                kprintln!();
                print_uptime();
            }
            // Control characters in caret notation (Backspace = ^H).
            Some(c) if c.is_ascii_control() && c != '\t' => {
                frame::kprint!("^{}", char::from(c as u8 ^ 0x40));
            }
            Some(c) => frame::kprint!("{c}"),
            None => {}
        }
    }
}

#[cfg(not(feature = "selftest"))]
fn print_uptime() {
    let ns = frame::time::now_ns();
    kprintln!(
        "uptime: {}.{:03} s, {} ticks",
        ns / 1_000_000_000,
        ns / 1_000_000 % 1_000,
        frame::time::ticks()
    );
}

#[panic_handler]
fn panic(panic: &core::panic::PanicInfo) -> ! {
    frame::report_panic(panic);
    #[cfg(feature = "selftest")]
    frame::qemu::exit(frame::qemu::ExitCode::Failure);
    #[cfg(not(feature = "selftest"))]
    frame::halt();
}
