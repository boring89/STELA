// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! proctest: processes, arguments and the heap. Started without
//! arguments, it starts copies of itself from `Initrd:/proctest`:
//!
//! - `proctest child <300 characters>`: checks its arguments (more than
//!   fit in a message) and exits with 42;
//! - `proctest wait`: waits on its main port for ever, until killed.
//!
//! Prints `proctest: OK` and exits with 0, or exits with the number of the
//! failed check.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use libstela::fs::{self, open};
use libstela::ipc::{FOREVER, port_receive};
use libstela::process::{self, standard_handles};
use libstela::{Error, Handle, Message, bootstrap, debug_println};

libstela::entry!(main);

fn long_arg() -> String {
    (0..300)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect()
}

fn check(step: i64, ok: bool, what: &str) -> Result<(), i64> {
    if ok {
        Ok(())
    } else {
        debug_println!("proctest: {what} FAILED");
        Err(step)
    }
}

fn spawn(me: Handle, args: &[&str]) -> Result<Handle, Error> {
    process::spawn(me, args, standard_handles(Handle::NONE, bootstrap::CWD))
}

fn parent() -> Result<(), i64> {
    // The heap: a 1 MiB vector, and many small allocations.
    let mut big: Vec<u32> = Vec::with_capacity(256 * 1024);
    for i in 0..256 * 1024u32 {
        big.push(i);
    }
    let sum: u64 = big.iter().map(|&x| u64::from(x)).sum();
    check(
        1,
        sum == (256 * 1024u64) * (256 * 1024 - 1) / 2,
        "1 MiB vector",
    )?;
    drop(big);
    let boxes: Vec<Box<[u8; 100]>> = (0..1000).map(|i| Box::new([i as u8; 100])).collect();
    check(
        2,
        boxes.iter().enumerate().all(|(i, b)| b[99] == i as u8),
        "small boxes",
    )?;
    drop(boxes);
    let mapped = libstela::heap::mapped();
    debug_println!("proctest: heap maps {} KiB", mapped / 1024);

    // Its own arguments: just the name.
    let args = process::args();
    check(3, args.len() == 1 && args[0] == "proctest", "own arguments")?;

    // A child with long arguments.
    let initrd = fs::volume_open("Initrd").map_err(|_| 4i64)?;
    let me = fs::open(initrd, "proctest", open::READ).map_err(|_| 4i64)?;
    let long = long_arg();
    let child = spawn(me, &["proctest", "child", &long]).map_err(|_| 5i64)?;
    check(5, process::wait_forever(child) == Ok(42), "child exit code")?;
    // Waiting again gives the same code.
    check(6, process::wait(child, 0) == Ok(42), "wait again")?;

    // A child that waits for ever: a timed-out wait, then a kill.
    let waiter = spawn(me, &["proctest", "wait"]).map_err(|_| 7i64)?;
    check(
        7,
        process::wait(waiter, 10_000_000) == Err(Error::TimedOut),
        "wait timeout",
    )?;
    let list = process::list().map_err(|_| 8i64)?;
    let running = list
        .iter()
        .filter(|p| p.name() == "proctest" && p.state == 1)
        .count();
    check(8, running == 2, "process list")?;
    process::kill(waiter).map_err(|_| 9i64)?;
    check(
        9,
        process::wait_forever(waiter) == Ok(libstela::abi::EXIT_KILLED),
        "kill",
    )?;

    // Not a program, and not a process handle.
    check(
        10,
        process::spawn(initrd, &["x"], [Handle::NONE; 3]) == Err(Error::IsDir),
        "spawn a folder",
    )?;
    check(
        11,
        process::wait(me, 0) == Err(Error::WrongType),
        "wait on a file",
    )?;
    let mine = process::this().map_err(|_| 12i64)?;
    check(
        12,
        process::wait(mine, 0) == Err(Error::AccessDenied),
        "wait on itself",
    )?;

    let mem = process::memory().map_err(|_| 13i64)?;
    check(
        13,
        mem.free_frames > 0 && mem.free_frames < mem.total_frames,
        "memory",
    )?;
    let (ns, _) = process::uptime().map_err(|_| 14i64)?;
    check(14, ns > 0, "uptime")?;
    Ok(())
}

fn main() -> i64 {
    let args = process::args();
    match args.get(1).map(String::as_str) {
        Some("child") => {
            if args.len() == 3 && args[2] == long_arg() {
                42
            } else {
                debug_println!("proctest: child got {} argument(s)", args.len());
                1
            }
        }
        Some("wait") => {
            let mut msg = Message::new();
            let _ = port_receive(bootstrap::MAIN_PORT, &mut msg, FOREVER);
            2
        }
        _ => match parent() {
            Ok(()) => {
                debug_println!("proctest: OK");
                0
            }
            Err(step) => step,
        },
    }
}
