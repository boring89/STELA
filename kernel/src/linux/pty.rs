// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pseudo-terminals for Linux programs (P12, `docs/linux.md` §5.4):
//! opening `/dev/ptmx` makes a `Tty` with a master side (the same object
//! STELA's terminal application gets from `TtyCreate(MASTER)`) and gives
//! it a number N; `/dev/pts/N` opens its program side.
//!
//! ```text
//!  open("/dev/ptmx") ── master fd: read = the program's output,
//!        │                         write = typed input (line discipline)
//!        │ TIOCGPTN → N
//!  open("/dev/pts/N") ── program fd (an ordinary tty descriptor)
//! ```
//!
//! The number is free again once nothing holds the `Tty`.

use crate::object::tty::Tty;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use frame::sync::SpinLock;

static PTYS: SpinLock<Vec<(u32, Weak<Tty>)>> = SpinLock::new(Vec::new());

/// A new pseudo-terminal and its number.
pub fn create() -> (u32, Arc<Tty>) {
    let tty = Tty::new(true);
    let mut ptys = PTYS.lock();
    ptys.retain(|(_, t)| t.strong_count() > 0);
    let mut n = 0;
    while ptys.iter().any(|(m, _)| *m == n) {
        n += 1;
    }
    ptys.push((n, Arc::downgrade(&tty)));
    (n, tty)
}

/// Pseudo-terminal `n`, if it exists.
pub fn find(n: u32) -> Option<Arc<Tty>> {
    PTYS.lock()
        .iter()
        .find(|(m, _)| *m == n)
        .and_then(|(_, t)| t.upgrade())
}

/// The numbers in use.
pub fn numbers() -> Vec<u32> {
    PTYS.lock()
        .iter()
        .filter(|(_, t)| t.strong_count() > 0)
        .map(|(n, _)| *n)
        .collect()
}
