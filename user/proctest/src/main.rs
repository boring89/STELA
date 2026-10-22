// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! proctest: processes, arguments and the heap. Started without
//! arguments, it starts copies of itself from `Initrd:/proctest`:
//!
//! - `proctest child <300 characters>`: checks its arguments (more than
//!   fit in a message) and exits with 42;
//! - `proctest wait`: waits on its main port for ever, until killed.
//!
//! Then it waits on a port, a terminal and a process with a wait set, and
//! joins threads.
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
use core::sync::atomic::{AtomicU32, Ordering};
use libstela::abi::wait::Ready;
use libstela::fs::{self, open};
use libstela::ipc::{FOREVER, port_create, port_receive, port_send};
use libstela::process::{self, standard_handles};
use libstela::wait::{self, WaitSet};
use libstela::{Error, Handle, Message, bootstrap, debug_println};
use libstela::{ThreadStack, tty};

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
    wait_sets(me)
}

/// The port the helper thread sends to (its raw handle).
static SEND_TO: AtomicU32 = AtomicU32::new(0);
static HELPER: ThreadStack<16384> = ThreadStack::new();

fn helper(_: u64) -> u64 {
    libstela::sleep_ms(5);
    let mut msg = Message::new();
    msg.tag = 77;
    let _ = port_send(
        Handle::from_raw(u64::from(SEND_TO.load(Ordering::Acquire))),
        &msg,
    );
    0
}

static WORKER: ThreadStack<16384> = ThreadStack::new();

/// Sleeps `arg` milliseconds and returns `arg + 1`.
fn worker(arg: u64) -> u64 {
    libstela::sleep_ms(arg);
    arg + 1
}

/// Wait sets: a port, a terminal's master and a process waited on
/// together; a wake-up from another thread.
fn wait_sets(me: Handle) -> Result<(), i64> {
    let set = WaitSet::new().map_err(|_| 15i64)?;
    let port = port_create().map_err(|_| 15i64)?;
    let (tty, master) = tty::create_with_master().map_err(|_| 15i64)?;
    let child = spawn(me, &["proctest", "wait"]).map_err(|_| 15i64)?;
    set.add(port, 1, 0).map_err(|_| 15i64)?;
    set.add(master, 2, wait::READABLE).map_err(|_| 15i64)?;
    set.add(child, 3, 0).map_err(|_| 15i64)?;
    let mut ready = [Ready::default(); 4];
    check(
        15,
        set.wait(&mut ready, 0) == Err(Error::TimedOut),
        "empty wait",
    )?;
    let mut msg = Message::new();
    msg.tag = 5;
    port_send(port, &msg).map_err(|_| 16i64)?;
    let n = set.wait(&mut ready, FOREVER).map_err(|_| 16i64)?;
    check(
        16,
        n == 1 && ready[0].key == 1 && ready[0].events == wait::READABLE,
        "port ready",
    )?;
    port_receive(port, &mut msg, 0).map_err(|_| 16i64)?;
    let text = Message::with_data(libstela::tag::CONSOLE_WRITE, b"x");
    port_send(tty, &text).map_err(|_| 17i64)?;
    let n = set.wait(&mut ready, FOREVER).map_err(|_| 17i64)?;
    check(17, n == 1 && ready[0].key == 2, "terminal ready")?;
    let mut buf = [0u8; 8];
    check(17, tty::read(master, &mut buf, 0) == Ok(1), "terminal read")?;
    process::kill(child).map_err(|_| 18i64)?;
    let n = set.wait(&mut ready, 1_000_000_000).map_err(|_| 18i64)?;
    check(
        18,
        n == 1 && ready[0].key == 3 && ready[0].events == wait::EXITED,
        "process ended",
    )?;
    set.remove(child).map_err(|_| 19i64)?;
    check(
        19,
        set.remove(child) == Err(Error::NotFound),
        "remove twice",
    )?;
    // Blocked in the wait, woken by a send from another thread.
    SEND_TO.store(port.raw() as u32, Ordering::Release);
    let helping = libstela::spawn(helper, 0, &HELPER).map_err(|_| 20i64)?;
    let start = libstela::now_ns();
    let n = set.wait(&mut ready, 2_000_000_000).map_err(|_| 20i64)?;
    check(
        20,
        n == 1 && ready[0].key == 1 && libstela::now_ns() - start >= 4_000_000,
        "woken by another thread",
    )?;
    port_receive(port, &mut msg, 0).map_err(|_| 20i64)?;
    check(20, msg.tag == 77, "helper's message")?;
    check(20, helping.join() == Ok(0), "helper joined")?;
    // A closed member is dropped: the wait times out.
    libstela::ipc::handle_close(port).map_err(|_| 21i64)?;
    check(
        21,
        set.wait(&mut ready, 2_000_000) == Err(Error::TimedOut),
        "closed member",
    )?;
    check(
        22,
        set.add(set.handle(), 4, 0) == Err(Error::WrongType),
        "set in itself",
    )?;
    check(
        22,
        set.add(master, 4, 1 << 20) == Err(Error::InvalidArgument),
        "bad mask",
    )?;
    // A port held only to send to can be watched for closing only.
    let other = port_create().map_err(|_| 23i64)?;
    let send_only =
        libstela::ipc::handle_duplicate(other, libstela::Rights::SEND).map_err(|_| 23i64)?;
    check(
        23,
        set.add(send_only, 5, wait::READABLE) == Err(Error::AccessDenied),
        "send-only port readable",
    )?;
    check(23, set.add(send_only, 5, 0).is_ok(), "send-only port")?;
    port_send(send_only, &msg).map_err(|_| 23i64)?;
    check(
        23,
        set.wait(&mut ready, 0) == Err(Error::TimedOut),
        "send-only port sees no messages",
    )?;
    capabilities()?;
    pipes(me)?;
    threads()
}

/// Thread handles: joined with a value, with a timeout, through a wait
/// set; a joined thread's stack can be used again.
fn threads() -> Result<(), i64> {
    let mut slow = libstela::spawn(worker, 50, &WORKER).map_err(|_| 29i64)?;
    check(
        29,
        libstela::ipc::handle_kind(slow.handle()) == Ok(libstela::abi::object_kind::THREAD),
        "kind",
    )?;
    check(
        29,
        libstela::spawn(worker, 1, &WORKER).err() == Some(Error::InvalidArgument),
        "stack in use",
    )?;
    check(
        29,
        slow.join_timeout(1_000_000) == Err(Error::TimedOut),
        "join times out",
    )?;
    check(29, slow.join_timeout(FOREVER) == Ok(51), "joined")?;
    // Joined again: the same value.
    check(29, slow.join() == Ok(51), "joined twice")?;
    let quick = libstela::spawn(worker, 5, &WORKER).map_err(|_| 30i64)?;
    let set = WaitSet::new().map_err(|_| 30i64)?;
    set.add(quick.handle(), 9, 0).map_err(|_| 30i64)?;
    let mut ready = [Ready::default(); 2];
    let n = set.wait(&mut ready, 2_000_000_000).map_err(|_| 30i64)?;
    check(
        30,
        n == 1 && ready[0].key == 9 && ready[0].events == wait::EXITED,
        "thread ended",
    )?;
    check(30, quick.join() == Ok(6), "value after the wait")?;
    Ok(())
}

/// Without the POWER capability (the self test gives none), SystemPower
/// is refused, and a bogus handle does not stand in for it.
fn capabilities() -> Result<(), i64> {
    check(
        24,
        process::capability(libstela::abi::capability::POWER).is_none(),
        "no POWER given",
    )?;
    check(
        24,
        process::shutdown() == Error::AccessDenied,
        "shutdown refused",
    )?;
    let raw = libstela::sys::syscall2(
        libstela::abi::Syscall::SystemPower,
        libstela::abi::power::REBOOT,
        bootstrap::MAIN_PORT.raw(),
    );
    check(24, raw == Err(Error::AccessDenied), "a port is not POWER")?;
    // Nor INPUT nor NETWORK.
    check(
        25,
        libstela::ipc::input_attach(bootstrap::MAIN_PORT) == Err(Error::AccessDenied),
        "input without INPUT",
    )?;
    check(
        25,
        libstela::net::Socket::udp().err() == Some(Error::AccessDenied),
        "socket without NETWORK",
    )?;
    // Nor another process by its id (P11, ProcessOpen).
    check(
        25,
        process::open(1) == Err(Error::AccessDenied),
        "open without POWER",
    )?;
    let raw = libstela::sys::syscall2(
        libstela::abi::Syscall::ProcessOpen,
        1,
        bootstrap::MAIN_PORT.raw(),
    );
    check(
        25,
        raw == Err(Error::AccessDenied),
        "a port is not POWER for open",
    )?;
    // P13 K11: no device and no network card without DEVICES.
    check(
        26,
        libstela::device::Device::open("00:02.0").err() == Some(Error::AccessDenied),
        "DeviceOpen without DEVICES",
    )?;
    let raw = libstela::sys::syscall3(
        libstela::abi::Syscall::DeviceOpen,
        b"00:02.0".as_ptr() as u64,
        7,
        bootstrap::MAIN_PORT.raw(),
    );
    check(26, raw == Err(Error::AccessDenied), "a port is not DEVICES")?;
    check(
        26,
        libstela::device::NetTap::attach(bootstrap::MAIN_PORT, [2, 0, 0, 0, 0, 1]).err()
            == Some(Error::AccessDenied),
        "NetAttach without DEVICES",
    )?;
    // P13 K15 (docs/sandbox.md): no other volume without VOLUMES; Initrd,
    // the read-only system files, is open to all.
    check(
        27,
        fs::volume_open("Data").err() == Some(Error::AccessDenied),
        "VolumeOpen without VOLUMES",
    )?;
    check(
        27,
        fs::volume_list(0).err() == Some(Error::AccessDenied),
        "VolumeList without VOLUMES",
    )?;
    let initrd = fs::volume_open("Initrd").map_err(|_| 27i64)?;
    let by_id = fs::open_by_id(initrd, fs::stat(initrd).map_or(0, |s| s.id));
    let _ = libstela::ipc::handle_close(initrd);
    check(
        27,
        by_id.err() == Some(Error::AccessDenied),
        "VolumeOpenById without VOLUMES",
    )?;
    let raw = libstela::sys::syscall3(
        libstela::abi::Syscall::VolumeOpen,
        b"Data".as_ptr() as u64,
        4,
        bootstrap::MAIN_PORT.raw(),
    );
    check(27, raw == Err(Error::AccessDenied), "a port is not VOLUMES")?;
    Ok(())
}

/// Pipes (P11): in order, the end of the stream, a writer without a
/// reader, a wait set, and a child printing into one as its [1].
fn pipes(me: Handle) -> Result<(), i64> {
    use libstela::pipe;
    let (r, w) = pipe::create().map_err(|_| 26i64)?;
    pipe::write_all(w, b"abc").map_err(|_| 26i64)?;
    pipe::write_all(w, b"def").map_err(|_| 26i64)?;
    let mut buf = [0u8; 64];
    check(26, pipe::read(r, &mut buf, 0) == Ok(6), "read")?;
    check(26, &buf[..6] == b"abcdef", "in order")?;
    check(
        26,
        pipe::read(r, &mut buf, 1_000_000) == Err(Error::TimedOut),
        "empty",
    )?;
    let set = WaitSet::new().map_err(|_| 26i64)?;
    set.add(r, 1, 0).map_err(|_| 26i64)?;
    let mut ready = [Ready::default(); 2];
    libstela::ipc::handle_close(w).map_err(|_| 26i64)?;
    check(
        26,
        set.wait(&mut ready, 0) == Ok(1) && ready[0].events == wait::READABLE | wait::CLOSED,
        "writer gone",
    )?;
    check(26, pipe::read(r, &mut buf, 0) == Ok(0), "end of the stream")?;
    let (r2, w2) = pipe::create().map_err(|_| 27i64)?;
    libstela::ipc::handle_close(r2).map_err(|_| 27i64)?;
    check(
        27,
        pipe::write(w2, b"x", 0) == Err(Error::PeerClosed),
        "no reader",
    )?;
    // A child whose [1] is a pipe: what it prints comes out here.
    let (r3, w3) = pipe::create().map_err(|_| 28i64)?;
    let out = process::pass(w3, libstela::Rights::SEND | libstela::Rights::WRITE);
    libstela::ipc::handle_close(w3).map_err(|_| 28i64)?;
    let child = process::spawn(
        me,
        &["proctest", "print"],
        [out, Handle::NONE, Handle::NONE],
    )
    .map_err(|_| 28i64)?;
    let mut got = [0u8; 128];
    let mut n = 0;
    loop {
        match pipe::read(r3, &mut got[n..], 2_000_000_000) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return Err(28),
        }
    }
    check(28, process::wait_forever(child) == Ok(0), "child")?;
    check(
        28,
        &got[..n] == b"proctest: through a pipe\n",
        "child's output",
    )?;
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
        Some("print") => {
            debug_println!("proctest: through a pipe");
            0
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
