// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The kernel console: `kprint!` and `kprintln!` write here. For now the
//! console is the serial port.

use core::fmt;

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    // Writing to the serial port cannot fail.
    let _ = fmt::Write::write_fmt(&mut crate::serial::Serial, args);
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
