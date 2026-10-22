// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! sandboxtest (`make test-sandbox`, P13 K17, `docs/sandbox.md`): put on
//! the test disk as `Data:/Sandbox.app` and started by Mouseion, it checks
//! that it runs in its container: its `[3]` is
//! `Data:/Programs/Sandbox/container/` and its own, its `[4]` (the bundle)
//! read only, no other volume but `Initrd`; then it asks for a file and
//! gets it through the window server's powerbox. Each step prints
//! `sandbox: ...`; the last `sandbox: OK` (or `sandbox: FAILED ...`).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use libstela::fs::{self, open};
use libstela::ipc::handle_close;
use libstela::path::Location;
use libstela::{Error, bootstrap, println};
use toolbox::dialog;
use toolbox::window::Connection;

libstela::entry!(main);

fn check(ok: bool, what: &str) -> Result<(), i64> {
    if ok {
        println!("sandbox: {what}");
        Ok(())
    } else {
        println!("sandbox: FAILED {what}");
        Err(1)
    }
}

fn run() -> Result<(), i64> {
    let here = fs::path_of(bootstrap::CWD).unwrap_or_default();
    check(
        here == "/Programs/Sandbox/container",
        &alloc::format!("working folder {here}"),
    )?;
    check(
        fs::volume_open("Data").err() == Some(Error::AccessDenied),
        "Data not allowed",
    )?;
    check(
        fs::volume_list(0).err() == Some(Error::AccessDenied),
        "volume list not allowed",
    )?;
    let id = fs::stat(bootstrap::CWD).map_or(0, |s| s.id);
    check(
        fs::open_by_id(bootstrap::CWD, id).err() == Some(Error::AccessDenied),
        "open by id not allowed",
    )?;
    let initrd = fs::volume_open("Initrd");
    check(initrd.is_ok(), "Initrd allowed")?;
    if let Ok(h) = initrd {
        let _ = handle_close(h);
    }
    // P14 K12: its info.txt says jit=yes.
    check(
        libstela::process::capability(libstela::abi::capability::JIT).is_some(),
        "JIT given (info.txt jit=yes)",
    )?;
    // P14 K4: the System volume too (read only, like Initrd).
    let system = fs::volume_open("System");
    check(system.is_ok(), "System allowed")?;
    if let Ok(h) = system {
        let _ = handle_close(h);
    }
    // The container is its own.
    let flags = open::READ | open::WRITE | open::CREATE | open::TRUNCATE;
    let note = fs::open(bootstrap::CWD, "note.txt", flags);
    let wrote = note.and_then(|h| {
        let r = fs::write(h, b"kept\n", 0);
        let _ = handle_close(h);
        r
    });
    check(wrote == Ok(5), "container writable")?;
    // Its bundle is read only.
    let exec = fs::open(bootstrap::PROGRAM, "exec", open::READ);
    check(exec.is_ok(), "bundle readable")?;
    if let Ok(h) = exec {
        let _ = handle_close(h);
    }
    let made = fs::open(
        bootstrap::PROGRAM,
        "x",
        open::READ | open::WRITE | open::CREATE,
    );
    check(made.is_err(), "bundle read only")?;
    // A file only through the powerbox.
    let mut conn = Connection::open().map_err(|_| 2i64)?;
    println!("sandbox: asking");
    let Some(picked) = dialog::pick_open(&mut conn, "Open", Some(&Location::root("Data"))) else {
        println!("sandbox: FAILED nothing picked");
        return Err(3);
    };
    let mut buf = vec![0u8; 256];
    let n = fs::read(picked.file, &mut buf, 0).unwrap_or(0);
    let _ = handle_close(picked.file);
    let text = String::from_utf8_lossy(&buf[..n]);
    println!(
        "sandbox: read {} ({n} bytes): {}",
        picked.location,
        text.trim_end()
    );
    Ok(())
}

fn main() -> i64 {
    match run() {
        Ok(()) => {
            println!("sandbox: OK");
            0
        }
        Err(code) => code,
    }
}
