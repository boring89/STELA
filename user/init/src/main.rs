// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! init: the first program.
//!
//! With a screen (the kernel gives init the framebuffer as `[2]`) and a
//! window server in the initrd, it starts `Initrd:/windowserver terminal`
//! with the framebuffer, the console and the root of `Data` (`Initrd`
//! when there is no `Data`), and starts it again if it ends.
//!
//! Otherwise (text mode) it starts `Initrd:/shell` with the console, a
//! `SEND` handle to its own main port (bootstrap `[2]`) and the same
//! working directory, waits for the shell to report in (`SHELL_READY`,
//! carrying a `SEND` handle to the shell's main port) and gives it the
//! keyboard. When the shell ends, init starts a new one.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::abi::object_kind;
use libstela::fs::{self, open};
use libstela::ipc::{handle_close, handle_duplicate, handle_kind, input_attach, port_receive};
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

/// The window server and the program it starts first.
const DESKTOP: &[&str] = &["windowserver", "Terminal.app"];

/// Runs the window server for as long as it keeps running; returns if it
/// cannot be started at all.
fn run_desktop(home: Handle) {
    let Ok(initrd) = fs::volume_open("Initrd") else {
        return;
    };
    let program = fs::open(initrd, "windowserver", open::READ);
    let _ = handle_close(initrd);
    let Ok(program) = program else {
        return;
    };
    loop {
        let handles = [
            pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
            pass(
                bootstrap::BOOTSTRAP,
                Rights::MAP | Rights::READ | Rights::WRITE | Rights::DUPLICATE,
            ),
            pass(home, Rights::READ | Rights::WRITE | Rights::DUPLICATE),
        ];
        let server = match process::spawn(program, DESKTOP, handles) {
            Ok(h) => h,
            Err(e) => {
                println!("init: cannot start the window server: {}", e.name());
                return;
            }
        };
        let code = process::wait_forever(server).unwrap_or(-1);
        let _ = handle_close(server);
        println!("init: the window server ended with code {code}; starting it again");
        libstela::sleep_ms(1000);
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
    if handle_kind(bootstrap::BOOTSTRAP) == Ok(object_kind::FRAMEBUFFER) {
        run_desktop(home);
        println!("init: no window server; text mode");
    }
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
