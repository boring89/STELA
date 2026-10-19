// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! x86-64 support.

pub(crate) mod io;

use core::arch::asm;

/// Disables maskable interrupts and halts forever.
pub(crate) fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli` and `hlt` only stop this processor; they touch no
        // memory. With interrupts off, `hlt` waits for an NMI at most, and the
        // loop halts again afterwards.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
