// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Leaving QEMU from a test run through the `isa-debug-exit` device
//! (`-device isa-debug-exit,iobase=0xf4,iosize=0x04`). Writing `v` to the
//! port makes QEMU exit with status `(v << 1) | 1` (QEMU source
//! `hw/misc/debugexit.c`).

use crate::arch::x86_64::io::outl;

/// The device's I/O port, as configured in the Makefile.
const PORT: u16 = 0xF4;

/// Exit status requested from QEMU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ExitCode {
    /// QEMU exits with 33.
    Success = 0x10,
    /// QEMU exits with 35.
    Failure = 0x11,
}

/// Ends the QEMU process. On a machine without the device the write does
/// nothing and the processor halts.
pub fn exit(code: ExitCode) -> ! {
    // SAFETY: port 0xF4 is reserved for isa-debug-exit in STELA's QEMU
    // setup; on real hardware it is unassigned and the write is ignored.
    unsafe { outl(PORT, code as u32) };
    crate::halt()
}
