// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The first STELA user program: prints a greeting, starts a second thread
//! that prints too, waits for it, and exits with code 0.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use libstela::{ThreadStack, debug_println};

libstela::entry!(main);

static STACK: ThreadStack<16384> = ThreadStack::new();
/// The argument the child thread saw, once it ran.
static CHILD_SAW: AtomicU64 = AtomicU64::new(0);

fn child(arg: u64) {
    debug_println!("hello from a second thread (argument {arg})");
    CHILD_SAW.store(arg, Ordering::Release);
}

fn main() -> i64 {
    debug_println!("hello from user mode");
    let start = libstela::now_ns();
    let id = match libstela::spawn(child, 7, &STACK) {
        Ok(id) => id,
        Err(e) => {
            debug_println!("hello: cannot start a thread: {}", e.name());
            return 1;
        }
    };
    while CHILD_SAW.load(Ordering::Acquire) == 0 {
        libstela::sleep_ms(1);
    }
    // The stack is used up: a second spawn on it is refused.
    if libstela::spawn(child, 8, &STACK).is_ok() {
        return 2;
    }
    debug_println!(
        "hello: thread {id} finished within {} us",
        (libstela::now_ns() - start) / 1000
    );
    0
}
