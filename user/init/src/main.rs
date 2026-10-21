// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! init: the first program.
//!
//! With a screen (the kernel gives init the framebuffer as `[2]`) and a
//! window server in the initrd, it starts `Initrd:/windowserver terminal`
//! with the framebuffer, the console and the root of `Data` (`Initrd`
//! when there is no `Data`), and starts it again if it ends.
//!
//! The boot options `textmode` and `nowindow` (`docs/p520.md`) keep it
//! in text mode even with a screen.
//!
//! Otherwise (text mode) it starts `Initrd:/shell` with the console, a
//! `SEND` handle to its own main port (bootstrap `[2]`) and the same
//! working directory, waits for the shell to report in (`SHELL_READY`,
//! carrying a `SEND` handle to the shell's main port) and gives it the
//! keyboard. When the shell ends, init starts a new one.
//!
//! The kernel gives init the capabilities `POWER`, `INPUT` and `NETWORK`
//! (P11, `docs/userland.md` §2.3). The window server gets all three (it
//! takes the input and passes `POWER` to Mouseion); the text-mode shell
//! gets `INPUT` (for `run keyecho`). Everyone also gets `INIT`, a `SEND`
//! handle to init's main port: while it waits for its child, init answers
//! `POWER_REQUEST`s there (the shell's `shutdown`).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::abi::{capability, object_kind};
use libstela::fs::{self, open};
use libstela::ipc::{
    FOREVER, handle_close, handle_duplicate, handle_kind, input_attach, port_receive, port_send,
};
use libstela::process::{self, pass};
use libstela::wait::{self, Ready, WaitSet};
use libstela::{Handle, Message, Rights, bootstrap, println, tag};

libstela::entry!(main);

/// How long to wait for a new shell to report in.
const READY_TIMEOUT: u64 = 5_000_000_000;

/// A message on init's main port other than the shell's report.
fn serve(msg: &Message) {
    if msg.tag == tag::POWER_REQUEST {
        let kind = msg.payload().get(..8).map_or(u64::MAX, |b| {
            u64::from_le_bytes(b.try_into().unwrap_or([0; 8]))
        });
        println!("init: power request {kind}");
        // Returns only if it failed.
        let e = process::power(kind);
        if let Some(&reply) = msg.carried().first() {
            let answer = Message::with_data(tag::POWER_REQUEST, &e.raw().to_le_bytes());
            let _ = port_send(reply, &answer);
        }
    }
    for &h in msg.carried() {
        let _ = handle_close(h);
    }
}

/// Waits for `child` to end, answering requests on the main port
/// meanwhile; its exit code.
fn wait_child(child: Handle) -> i64 {
    let Ok(set) = WaitSet::new() else {
        return process::wait_forever(child).unwrap_or(-1);
    };
    if set.add(child, 1, wait::EXITED).is_err()
        || set.add(bootstrap::MAIN_PORT, 0, wait::READABLE).is_err()
    {
        return process::wait_forever(child).unwrap_or(-1);
    }
    let mut ready = [Ready::default(); 2];
    loop {
        let Ok(n) = set.wait(&mut ready, FOREVER) else {
            continue;
        };
        for r in &ready[..n] {
            if r.key == 1 {
                return process::wait(child, 0).unwrap_or(-1);
            }
            let mut msg = Message::new();
            while port_receive(bootstrap::MAIN_PORT, &mut msg, 0).is_ok() {
                serve(&msg);
            }
        }
    }
}

/// A copy of capability `kind` for a child.
fn give(kind: u32) -> Option<(u32, Handle)> {
    process::pass_capability(kind).map(|h| (kind, h))
}

fn start_shell(program: Handle, home: Handle) -> Option<Handle> {
    let to_init = handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND | Rights::TRANSFER).ok()?;
    let handles = [
        pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
        to_init,
        process::pass_dir(home),
    ];
    let mut caps = process::inherited();
    caps.extend(give(capability::INPUT));
    let shell = match process::spawn_with(program, &["shell"], &handles, &caps) {
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
            Ok(()) => serve(&msg),
            Err(e) => {
                println!("init: the shell did not report in: {}", e.name());
                return Some(shell);
            }
        }
    }
}

/// The window server and the program it starts first.
const DESKTOP: &[&str] = &["windowserver", "mouseion"];
/// The first start (P12, docs/setup.md): Setup.app instead of Mouseion;
/// Setup starts Mouseion when it is done.
const FIRST_START: &[&str] = &["windowserver", "Setup.app"];

/// Whether to run Setup.app: the boot option `setup`, or a Data volume
/// without `.stela/setup-done` (no Data at all: nothing to set up).
fn needs_setup() -> bool {
    if process::boot_flag("setup") {
        return true;
    }
    let Ok(data) = fs::volume_open("Data") else {
        return false;
    };
    let done = fs::open(data, ".stela/setup-done", open::READ);
    let _ = handle_close(data);
    match done {
        Ok(h) => {
            let _ = handle_close(h);
            false
        }
        Err(_) => true,
    }
}

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
    let mut programs = if needs_setup() {
        println!("init: first start: Setup.app");
        FIRST_START
    } else {
        DESKTOP
    };
    loop {
        let handles = [
            pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
            pass(
                bootstrap::BOOTSTRAP,
                Rights::MAP | Rights::READ | Rights::WRITE | Rights::DUPLICATE,
            ),
            process::pass_dir(home),
        ];
        let mut caps = process::inherited();
        caps.extend(give(capability::POWER));
        caps.extend(give(capability::INPUT));
        let server = match process::spawn_with(program, programs, &handles, &caps) {
            Ok(h) => h,
            Err(e) => {
                println!("init: cannot start the window server: {}", e.name());
                return;
            }
        };
        let code = wait_child(server);
        let _ = handle_close(server);
        println!("init: the window server ended with code {code}; starting it again");
        programs = DESKTOP;
        libstela::sleep_ms(1000);
    }
}

fn main() -> i64 {
    match handle_duplicate(
        bootstrap::MAIN_PORT,
        Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
    ) {
        Ok(h) => process::set_capability(capability::INIT, h),
        Err(e) => println!("init: no INIT capability: {}", e.name()),
    }
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
    let text_only = process::boot_flag("textmode") || process::boot_flag("nowindow");
    if text_only {
        println!("init: text mode (boot option)");
    } else if handle_kind(bootstrap::BOOTSTRAP) == Ok(object_kind::FRAMEBUFFER) {
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
        let code = wait_child(shell);
        let _ = handle_close(shell);
        println!("init: the shell ended with code {code}; starting a new one");
    }
}
