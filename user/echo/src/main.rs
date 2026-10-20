// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! echo: attaches its main port to the keyboard, prints what is typed
//! (control characters in caret notation) and the uptime on Enter; Escape
//! ends it.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::abi::key::{KeyEvent, key_to_char};
use libstela::ipc::{FOREVER, input_attach, port_receive};
use libstela::{Message, bootstrap, debug_print, debug_println, tag};

libstela::entry!(main);

fn main() -> i64 {
    if let Err(e) = input_attach(bootstrap::MAIN_PORT) {
        debug_println!("echo: cannot attach to the keyboard: {}", e.name());
        return 1;
    }
    debug_println!("echo: ready; type to echo, Enter prints the uptime, Escape ends");
    let mut msg = Message::new();
    loop {
        if let Err(e) = port_receive(bootstrap::MAIN_PORT, &mut msg, FOREVER) {
            debug_println!("echo: receive failed: {}", e.name());
            return 1;
        }
        if msg.tag != tag::INPUT_KEY {
            continue;
        }
        let Some(c) = KeyEvent::decode(msg.payload()).and_then(|e| key_to_char(&e)) else {
            continue;
        };
        match c {
            '\u{1b}' => {
                debug_println!();
                debug_println!("echo: bye");
                return 0;
            }
            '\n' => {
                let ns = libstela::now_ns();
                debug_println!();
                debug_println!(
                    "uptime: {}.{:03} s",
                    ns / 1_000_000_000,
                    ns / 1_000_000 % 1_000
                );
            }
            // Control characters in caret notation (Backspace = ^H).
            c if c.is_ascii_control() && c != '\t' => {
                debug_print!("^{}", char::from(c as u8 ^ 0x40));
            }
            c => debug_print!("{c}"),
        }
    }
}
