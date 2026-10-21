// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! bench: measures what `make bench` reports (`docs/bench.md`), one line
//! each, then `bench: done`:
//!
//! - the memory in use when it starts (on an idle desktop);
//! - a system call's round trip (`ClockNowNs`, 100,000 times);
//! - thread switches per second (two threads passing a message back and
//!   forth over two ports);
//! - eHFS: writing and reading 16 MiB sequentially (`Data:/bench.tmp`,
//!   removed afterwards);
//! - TCP: downloading `URL` (the argument, `http://host:port/path`) if
//!   one is given.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use libstela::fs::{self, open};
use libstela::ipc::{FOREVER, handle_close, port_create, port_receive, port_send};
use libstela::net::{self, Socket};
use libstela::{Handle, Message, ThreadStack, println, process};

libstela::entry!(main);

/// Round trips of the switch test.
const ROUND_TRIPS: u64 = 10_000;
/// The file test's size.
const FILE_BYTES: usize = 16 << 20;
/// Bytes a write or read asks for.
const CHUNK: usize = 64 * 1024;

/// `bytes` in `ns` as MB/s (10^6 bytes) with one decimal.
fn rate(bytes: u64, ns: u64) -> alloc::string::String {
    let tenths = (u128::from(bytes) * 10_000 / u128::from(ns.max(1))) as u64;
    alloc::format!("{}.{}", tenths / 10, tenths % 10)
}

fn memory() {
    match process::memory() {
        Ok(m) => println!(
            "bench: memory used {} KiB of {} KiB, kernel heap {} KiB used of {} KiB",
            (m.total_frames - m.free_frames) * 4,
            m.total_frames * 4,
            m.heap_used / 1024,
            m.heap_size / 1024
        ),
        Err(e) => println!("bench: memory: {}", e.name()),
    }
}

fn syscalls() {
    const CALLS: u64 = 100_000;
    let start = libstela::now_ns();
    let mut last = 0;
    for _ in 0..CALLS {
        last = libstela::now_ns();
    }
    let ns = last.saturating_sub(start);
    println!(
        "bench: syscall {} ns (ClockNowNs, {CALLS} calls)",
        ns / CALLS
    );
}

static PONG: ThreadStack<16384> = ThreadStack::new();
/// The ports of the switch test: (ping, pong), as raw handles.
static PORTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The other side: answers every message on `ping` on `pong` until a
/// message with tag 0.
fn pong(_: u64) -> u64 {
    let ports = PORTS.load(core::sync::atomic::Ordering::Acquire);
    let (ping, pong) = (
        Handle::from_raw(ports >> 32),
        Handle::from_raw(ports & 0xFFFF_FFFF),
    );
    let mut msg = Message::new();
    loop {
        if port_receive(ping, &mut msg, FOREVER).is_err() || msg.tag == 0 {
            return 0;
        }
        if port_send(pong, &msg).is_err() {
            return 1;
        }
    }
}

fn switches() {
    let (Ok(ping), Ok(pong_port)) = (port_create(), port_create()) else {
        println!("bench: switch: no ports");
        return;
    };
    PORTS.store(
        ping.raw() << 32 | pong_port.raw(),
        core::sync::atomic::Ordering::Release,
    );
    let Ok(thread) = libstela::spawn(pong, 0, &PONG) else {
        println!("bench: switch: no thread");
        return;
    };
    let mut msg = Message::new();
    msg.tag = 1;
    let start = libstela::now_ns();
    for _ in 0..ROUND_TRIPS {
        if port_send(ping, &msg).is_err() || port_receive(pong_port, &mut msg, FOREVER).is_err() {
            println!("bench: switch: the message got lost");
            return;
        }
    }
    let ns = libstela::now_ns().saturating_sub(start).max(1);
    msg.tag = 0;
    let _ = port_send(ping, &msg);
    let _ = thread.join();
    let switches = 2 * ROUND_TRIPS;
    println!(
        "bench: switch {} per second (two threads, port ping-pong, {switches} switches)",
        u128::from(switches) * 1_000_000_000 / u128::from(ns)
    );
    let _ = handle_close(ping);
    let _ = handle_close(pong_port);
}

fn files() {
    let Ok(data) = fs::volume_open("Data") else {
        println!("bench: ehfs: no Data volume");
        return;
    };
    let file = match fs::open(
        data,
        "bench.tmp",
        open::READ | open::WRITE | open::CREATE | open::TRUNCATE,
    ) {
        Ok(f) => f,
        Err(e) => {
            println!("bench: ehfs: {}", e.name());
            return;
        }
    };
    let mut buf = vec![0u8; CHUNK];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i * 7) as u8;
    }
    let start = libstela::now_ns();
    let mut done = 0;
    while done < FILE_BYTES {
        match fs::write(file, &buf, done as u64) {
            Ok(n) if n > 0 => done += n,
            _ => break,
        }
    }
    let write_ns = libstela::now_ns().saturating_sub(start);
    let written = done;
    let start = libstela::now_ns();
    let mut done = 0;
    while done < written {
        match fs::read(file, &mut buf, done as u64) {
            Ok(n) if n > 0 => done += n,
            _ => break,
        }
    }
    let read_ns = libstela::now_ns().saturating_sub(start);
    let _ = handle_close(file);
    let _ = fs::remove(data, "bench.tmp");
    let _ = handle_close(data);
    println!(
        "bench: ehfs write {} MB/s, read {} MB/s ({} KiB)",
        rate(written as u64, write_ns),
        rate(done as u64, read_ns),
        written / 1024
    );
}

fn download(url: &str) {
    let Some((host, port, path)) = net::parse_url(url) else {
        println!("bench: tcp: not an http:// URL: {url}");
        return;
    };
    if net::wait_configured(10_000_000_000).is_none() {
        println!("bench: tcp: no network");
        return;
    }
    let result = (|| {
        let ip = net::resolve(host)?;
        let s = Socket::tcp()?;
        let start = libstela::now_ns();
        s.connect(ip, port, 5_000_000_000)?;
        let request = alloc::format!("GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n");
        s.send_all(request.as_bytes())?;
        let mut buf = vec![0u8; CHUNK];
        let mut bytes = 0u64;
        loop {
            let (n, ..) = s.recv(&mut buf, 5_000_000_000)?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
        }
        Ok::<_, libstela::Error>((bytes, libstela::now_ns().saturating_sub(start)))
    })();
    match result {
        Ok((bytes, ns)) => println!("bench: tcp {} MB/s ({bytes} bytes)", rate(bytes, ns)),
        Err(e) => println!("bench: tcp: {}", e.name()),
    }
}

fn main() -> i64 {
    let args = process::args();
    memory();
    syscalls();
    switches();
    files();
    if let Some(url) = args.get(1) {
        download(url);
    }
    println!("bench: done");
    0
}
