// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! init: the first program. It starts `Initrd:/shell` with the console,
//! a `SEND` handle to its own main port (bootstrap `[2]`) and the root of
//! `Data` as working directory (`Initrd` when there is no `Data`), waits
//! for the shell to report in (`SHELL_READY`, carrying a `SEND` handle to
//! the shell's main port) and gives it the keyboard. When the shell ends,
//! init starts a new one.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::fs::{self, open};
use libstela::ipc::{handle_close, handle_duplicate, input_attach, port_receive};
use libstela::process::{self, pass};
use libstela::{Handle, Message, Rights, bootstrap, println, tag};

libstela::entry!(main);

/// How long to wait for a new shell to report in.
const READY_TIMEOUT: u64 = 5_000_000_000;

fn start_shell(program: Handle, home: Handle) -> Option<Handle> {
    let to_init = handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND | Rights::TRANSFER).ok()?;
    let handles = [
        pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
        to_init,
        pass(home, Rights::READ | Rights::WRITE | Rights::DUPLICATE),
    ];
    let shell = match process::spawn(program, &["shell"], handles) {
        Ok(h) => h,
        Err(e) => {
            println!("init: cannot start the shell: {}", e.name());
            return None;
        }
    };
    let mut msg = Message::new();
    loop {
        match port_receive(bootstrap::MAIN_PORT, &mut msg, READY_TIMEOUT) {
            Ok(()) if msg.tag == tag::SHELL_READY => {
                if let Some(&port) = msg.carried().first() {
                    let _ = input_attach(port);
                    let _ = handle_close(port);
                }
                return Some(shell);
            }
            Ok(()) => {
                // Something else: release what it carried.
                for &h in msg.carried() {
                    let _ = handle_close(h);
                }
            }
            Err(e) => {
                println!("init: the shell did not report in: {}", e.name());
                return Some(shell);
            }
        }
    }
}

fn main() -> i64 {
    let home = match fs::volume_open("Data") {
        Ok(h) => h,
        Err(_) => match fs::volume_open("Initrd") {
            Ok(h) => h,
            Err(e) => {
                println!("init: no volume to start in: {}", e.name());
                return 1;
            }
        },
    };
    let program = match fs::volume_open("Initrd").and_then(|i| fs::open(i, "shell", open::READ)) {
        Ok(h) => h,
        Err(e) => {
            println!("init: no Initrd:/shell: {}", e.name());
            return 1;
        }
    };
    loop {
        let Some(shell) = start_shell(program, home) else {
            libstela::sleep_ms(1000);
            continue;
        };
        let code = process::wait_forever(shell).unwrap_or(-1);
        let _ = handle_close(shell);
        println!("init: the shell ended with code {code}; starting a new one");
    }
}
