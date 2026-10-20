// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The kernel console: `kprint!` and `kprintln!` write to the serial port
//! and, once [`init`](crate::init) has found a framebuffer, to the screen.

#[cfg(target_os = "none")]
pub(crate) mod fb;
pub mod font;
pub mod text;

#[cfg(target_os = "none")]
pub use fb::{FramebufferDevice, device as framebuffer, hand_over, handed_over};

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

#[doc(hidden)]
#[cfg(target_os = "none")]
pub fn _print(args: fmt::Arguments) {
    let _guard = PRINT.lock();
    // Neither output can fail.
    let _ = fmt::Write::write_fmt(&mut Both, args);
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
