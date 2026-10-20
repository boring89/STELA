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
    // The stela, until the window server takes the screen (or, in text
    // mode, for a moment); boot messages go on to the serial port.
    frame::console::boot_screen();
    kernel::task::init();
    kernel::syscall::init();
    kernel::console::start();
    drivers::pci::system::init();
    kernel::block::probe();
    kernel::usb::start();
    // The self test drives the card raw first, then starts the stack.
    #[cfg(not(feature = "selftest"))]
    kernel::net::start();
    kernel::vfs::mount::mount_all();
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

/// The first kernel thread: starts the input router, then runs
/// `Initrd:/init` (which starts the window server, or the shell) and
/// waits for it.
#[cfg(not(feature = "selftest"))]
fn init() {
    kernel::input::start();
    if let Err(e) = kernel::task::spawn("boot-screen", end_boot_screen) {
        kprintln!("kernel: no boot screen watcher: {e:?}");
    }
    loop {
        run("init");
        // init never ends on its own; start it again if it was killed.
        kernel::task::sleep_ms(1000);
    }
}

/// Text mode: when nothing took the screen within 5 seconds (at once with
/// the boot option `textmode` or `nowindow`), the boot screen gives way to
/// the text console.
#[cfg(not(feature = "selftest"))]
fn end_boot_screen() {
    let wait = if frame::boot_flag("textmode") || frame::boot_flag("nowindow") {
        0
    } else {
        50
    };
    for _ in 0..wait {
        if frame::console::handed_over() {
            return;
        }
        kernel::task::sleep_ms(100);
    }
    frame::console::end_boot_screen();
}

/// Runs program `name` from the `Initrd` volume and waits for it to end.
/// Its bootstrap handle `[2]` is the framebuffer, when there is one
/// (`docs/display.md`).
#[cfg(not(feature = "selftest"))]
fn run(name: &str) {
    use abi::Rights;
    use kernel::object::{Entry, KernelObject};
    let framebuffer = frame::console::framebuffer().map(|fb| Entry {
        object: KernelObject::Framebuffer(alloc::sync::Arc::new(fb)),
        rights: Rights::MAP | Rights::READ | Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER,
    });
    match kernel::program::spawn_path("Initrd", name, framebuffer) {
        Ok(process) => {
            let pid = process.id();
            drop(process);
            let code = kernel::task::wait_process(pid).unwrap_or(-1);
            kprintln!("kernel: {name} (process {pid}) exited with code {code}");
        }
        Err(e) => kprintln!("kernel: cannot start {name} from the initrd: {e:?}"),
    }
}

#[panic_handler]
fn panic(panic: &core::panic::PanicInfo) -> ! {
    frame::report_panic(panic);
    // The panic screen test keeps QEMU running so that the screen can be
    // saved (`make test-panic`).
    #[cfg(feature = "selftest")]
    if !selftest::panic_test() {
        frame::qemu::exit(frame::qemu::ExitCode::Failure);
    }
    frame::halt();
}
