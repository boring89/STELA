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

extern crate alloc;

pub mod acpi;
#[cfg(target_os = "none")]
pub mod arch;
pub mod console;
#[cfg(target_os = "none")]
pub mod cpu;
pub mod cpuid;
pub mod input;
#[cfg(target_os = "none")]
pub mod irq;
pub mod mm;
#[cfg(target_os = "none")]
pub mod pci;
#[cfg(target_os = "none")]
pub mod power;
#[cfg(target_os = "none")]
pub mod qemu;
pub mod random;
#[cfg(target_os = "none")]
pub mod serial;
#[cfg(target_os = "none")]
pub mod smp;
pub mod sync;
#[cfg(target_os = "none")]
pub mod syscall;
#[cfg(target_os = "none")]
pub mod task;
pub mod time;
pub mod trap;

/// Brings up what the kernel relies on: serial console, GDT, TSS and IDT,
/// the frame allocator, the page tables, the heap, the framebuffer console,
/// the ACPI MADT, the interrupt controllers (8259s masked, local APIC, I/O
/// APICs), the per-CPU area and the boot thread's context, the PS/2
/// keyboard and the 1000 Hz timer.
/// Interrupts are enabled on return. Call
/// once, first thing in the kernel.
#[cfg(target_os = "none")]
pub fn init(info: &'static bootinfo::BootInfo) {
    let _ = BOOT_INFO.set(info);
    serial::init();
    // With `verbose`, every step says when it starts and ends
    // (`console::step`); the console reaches the screen from `fb::init`.
    use console::step;
    step("cpu tables (GDT, TSS, IDT)", arch::x86_64::init);
    step("PAT", arch::x86_64::pat::init);
    step("memory", || mm::init(info));
    step("screen", || console::fb::init(info));
    step("ACPI", || acpi::init(info));
    step("interrupt controllers", || irq::init(acpi::madt()));
    step("per-CPU area", task::init);
    arch::x86_64::syscall::init();
    arch::x86_64::enable_smep();
    step("keyboard and mouse (PS/2)", input::init);
    step("timer", time::init);
    irq::enable();
    console::KEYS_READY.store(true, core::sync::atomic::Ordering::Release);
}

/// The boot information, kept for later readers (it lives in
/// `KernelAndBoot` memory, which is never reused).
#[cfg(target_os = "none")]
static BOOT_INFO: sync::Once<&'static bootinfo::BootInfo> = sync::Once::new();

/// The boot options from `\\STELA\\boot.cfg` (space-separated `key` or
/// `key=value` words; `docs/p520.md`); empty before [`init`].
#[cfg(target_os = "none")]
pub fn cmdline() -> &'static [u8] {
    BOOT_INFO.get().map_or(&[], |info| info.cmdline())
}

/// The loader's memory map (merged regions; `docs/boot.md`); empty
/// before [`init`].
#[cfg(target_os = "none")]
pub fn memory_regions() -> &'static [bootinfo::MemoryRegion] {
    BOOT_INFO.get().map_or(&[], |info| info.regions())
}

/// The framebuffer the loader set up (`Framebuffer::NONE` when there is
/// none, or before [`init`]).
#[cfg(target_os = "none")]
pub fn boot_framebuffer() -> bootinfo::Framebuffer {
    BOOT_INFO
        .get()
        .map_or(bootinfo::Framebuffer::NONE, |info| info.framebuffer)
}

/// Whether the boot options set `key` ([`abi::sysinfo::cmdline_flag`]).
#[cfg(target_os = "none")]
pub fn boot_flag(key: &str) -> bool {
    abi::sysinfo::cmdline_flag(cmdline(), key)
}

/// Physical address of the page below 1 MiB the loader reserved for the
/// application processors' start-up code; 0 if there is none.
#[cfg(target_os = "none")]
pub fn ap_trampoline_phys() -> u64 {
    BOOT_INFO.get().map_or(0, |info| info.ap_trampoline_phys)
}

/// Enables interrupts and waits for the next one (`sti; hlt`: `sti` takes
/// effect after the next instruction, so an interrupt cannot slip in
/// between and be missed by the `hlt`). Returns after that interrupt's
/// handler has run, with interrupts enabled.
#[cfg(target_os = "none")]
pub fn halt_until_interrupt() {
    arch::x86_64::halt_until_interrupt();
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

/// Reports a panic: the message and location go to the serial port, and the
/// framebuffer shows the panic screen. The kernel's `#[panic_handler]` calls
/// this, then halts (or, in a self test, exits QEMU).
#[cfg(target_os = "none")]
pub fn report_panic(info: &core::panic::PanicInfo) {
    use core::fmt::Write;
    // Straight to the serial port: the console lock may be held by the code
    // that panicked.
    let mut serial = serial::Serial;
    let _ = writeln!(serial, "\n*** STELA kernel panic ***");
    if let Some(location) = info.location() {
        let _ = writeln!(
            serial,
            "at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        );
    }
    let _ = writeln!(serial, "{}", info.message());
    match info.location() {
        Some(l) => console::fb::panic_screen(format_args!(
            "{}\n\nat {}:{}:{}",
            info.message(),
            l.file(),
            l.line(),
            l.column()
        )),
        None => console::fb::panic_screen(format_args!("{}", info.message())),
    }
    let _ = writeln!(serial, "(the panic screen is up)");
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
