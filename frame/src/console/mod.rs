// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The kernel console: `kprint!` and `kprintln!` write to the serial port
//! and, once [`init`](crate::init) has found a framebuffer, to the screen.

pub mod art;
#[cfg(target_os = "none")]
pub(crate) mod fb;
pub mod font;
pub mod ring;
pub mod text;

#[cfg(target_os = "none")]
pub use fb::{
    FramebufferDevice, boot_screen, device as framebuffer, end_boot_screen, hand_over, handed_over,
    ink_rows,
};

#[cfg(target_os = "none")]
use core::fmt;

#[cfg(target_os = "none")]
struct Both;

#[cfg(target_os = "none")]
impl fmt::Write for Both {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        crate::serial::write_str(s);
        fb::write_str(s);
        Ok(())
    }
}

/// Keeps one `kprint!` from being interleaved with another: with threads
/// and preemption, two lines could otherwise mix on the serial port. Held
/// with interrupts disabled, so an interrupt handler cannot run into it on
/// the same processor. Panics bypass it (they write the serial port
/// directly).
#[cfg(target_os = "none")]
static PRINT: crate::sync::SpinLock<()> = crate::sync::SpinLock::new(());

/// The last lines printed, for the panic screen (P11).
#[cfg(target_os = "none")]
pub(crate) static RING: crate::sync::SpinLock<ring::LineRing> =
    crate::sync::SpinLock::new(ring::LineRing::new());

#[cfg(target_os = "none")]
impl fmt::Write for &mut ring::LineRing {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        ring::LineRing::write(self, s);
        Ok(())
    }
}

#[doc(hidden)]
#[cfg(target_os = "none")]
pub fn _print(args: fmt::Arguments) {
    let _guard = PRINT.lock();
    // Neither output can fail.
    let _ = fmt::Write::write_fmt(&mut Both, args);
    if let Some(mut ring) = RING.try_lock() {
        let _ = fmt::Write::write_fmt(&mut &mut *ring, args);
    }
}

/// Runs one step of the boot (P11, `docs/p520.md` §3.1): with the boot
/// option `verbose`, a line before and after it (with the time it took);
/// with `waitkey`, a pause after it until a key is pressed (5 seconds at
/// most), once the keyboard and the timer work.
#[cfg(target_os = "none")]
pub fn step<R>(name: &str, f: impl FnOnce() -> R) -> R {
    let verbose = crate::boot_flag("verbose");
    let start = crate::time::now_ns();
    if verbose {
        crate::kprintln!("boot: {name} ...");
    }
    let r = f();
    if verbose {
        let us = crate::time::now_ns().saturating_sub(start) / 1000;
        crate::kprintln!("boot: {name} done ({}.{:03} ms)", us / 1000, us % 1000);
    }
    if crate::boot_flag("waitkey") && KEYS_READY.load(core::sync::atomic::Ordering::Acquire) {
        wait_key();
    }
    r
}

/// Set once interrupts, the timer and the keyboard work (the end of
/// `frame::init`): only then can `waitkey` wait.
#[cfg(target_os = "none")]
pub(crate) static KEYS_READY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Waits for a key press (PS/2 at once; USB once its thread runs), five
/// seconds at most (a machine without a keyboard still boots).
#[cfg(target_os = "none")]
fn wait_key() {
    crate::kprintln!("boot: press any key (5 s)");
    let end = crate::time::now_ns() + 5_000_000_000;
    while crate::time::now_ns() < end {
        while let Some(event) = crate::input::pop() {
            if let crate::input::InputEvent::Key(k) = event
                && k.pressed
            {
                return;
            }
        }
        crate::halt_until_interrupt();
    }
    crate::kprintln!("boot: no key; going on");
}

/// Prints to the kernel console.
#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::console::_print(format_args!($($arg)*))
    };
}

/// Prints to the kernel console, followed by a newline.
#[macro_export]
macro_rules! kprintln {
    () => {
        $crate::kprint!("\n")
    };
    ($($arg:tt)*) => {
        $crate::console::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
