// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! powerbox (`Initrd:/powerbox`, P13 K16, `docs/sandbox.md`): the file
//! dialog for a program that runs in its container. The window server
//! starts it for a `POWERBOX` request with the `VOLUMES` capability:
//!
//! ```text
//!  powerbox open|save TITLE NAME START
//!  [2]  the window server's port (to show the dialog)
//!  [6]  SEND to the asking program's main port
//! ```
//!
//! It shows the toolbox's Open or Save dialog, opens (or, to save, makes)
//! the file the user picked and sends the program a `POWERBOX_RESULT`
//! carrying it: `READ`, and `WRITE` when the volume is writable, never
//! `MANAGE`. The program never sees the volumes; it gets that one file.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::window::{self as proto, powerbox};
use alloc::format;
use libstela::ipc::{handle_close, port_send};
use libstela::path::{self, Location};
use libstela::process::{self, pass};
use libstela::{Handle, Rights, bootstrap, println};
use toolbox::dialog;
use toolbox::window::Connection;

libstela::entry!(main);

/// A copy of `file` for the program: `READ | WRITE` if it has both, else
/// `READ` (and `DUPLICATE`, to keep a copy).
fn for_program(file: Handle) -> Handle {
    [Rights::READ | Rights::WRITE, Rights::READ]
        .into_iter()
        .map(|r| pass(file, r | Rights::DUPLICATE))
        .find(|&c| c != Handle::NONE)
        .unwrap_or(Handle::NONE)
}

fn main() -> i64 {
    let args = process::args();
    let arg = |i: usize| args.get(i).map_or("", |s| s.as_str());
    let save = arg(1) == "save";
    let reply = bootstrap::DOCUMENT;
    let cancelled = || {
        let _ = port_send(reply, &proto::powerbox_result(powerbox::CANCELLED, ""));
    };
    let mut conn = match Connection::open() {
        Ok(c) => c,
        Err(e) => {
            println!("powerbox: no window server: {}", e.name());
            cancelled();
            return 1;
        }
    };
    let start = Some(arg(4))
        .filter(|s| !s.is_empty())
        .and_then(|s| path::resolve(&Location::root("Data"), s).ok());
    let title = if arg(2).is_empty() {
        if save { "Save" } else { "Open" }
    } else {
        arg(2)
    };
    let mode = if save { "save" } else { "open" };
    let name = save.then(|| arg(3));
    let picked = dialog::file_dialog(&mut conn, title, start.as_ref(), name, &|| {
        println!("powerbox: {mode} dialog shown");
    });
    let Some(loc) = picked else {
        println!("powerbox: cancelled");
        cancelled();
        return 0;
    };
    let file = match dialog::open_location(&loc, save) {
        Ok(f) => f,
        Err(e) => {
            println!("powerbox: {loc}: {}", e.name());
            cancelled();
            return 0;
        }
    };
    let copy = for_program(file);
    let _ = handle_close(file);
    let mut msg = proto::powerbox_result(powerbox::PICKED, &format!("{loc}"));
    msg.push_handle(copy);
    match port_send(reply, &msg) {
        Ok(()) => println!("powerbox: picked {loc}"),
        Err(e) => {
            println!("powerbox: the program is gone: {}", e.name());
            let _ = handle_close(copy);
        }
    }
    0
}
