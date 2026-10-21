// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! stress (P11 K46, `docs/testing.md` §4): the system busy with many
//! things at once, and nothing left behind. Each round:
//!
//! ```text
//!  procs   100 processes (Initrd:/bin/true) started together, then waited
//!  files   1000 files made (100 bytes each) in Data:/stress, then removed
//!  pipe    64 MiB through a pipe: a writer thread, this thread reads
//!  tcp     50 threads, each a TCP connection downloading the same file
//!  windows 20 windows, each filled and damaged 20 times, then closed
//! ```
//!
//! The first two rounds warm up (caches, the heaps' free lists, the
//! handle table); after the third the free frames, the kernel heap, the
//! handle table and the running processes must be where the second left
//! them. From the second round on each part prints what it changed.
//! `stress [url]`: without a URL the TCP part is skipped.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use libstela::abi::kind;
use libstela::fs::{self, open};
use libstela::ipc::{handle_close, handle_duplicate};
use libstela::sync::SpinMutex;
use libstela::{Handle, Rights, ThreadStack, bootstrap, println, process};

libstela::entry!(main);

/// A handle closed when dropped.
struct Owned(Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = handle_close(self.0);
    }
}

const PROCS: usize = 100;
const FILES: usize = 1000;
const PIPE_BYTES: usize = 64 << 20;
const CHUNK: usize = 64 * 1024;
const CONNECTIONS: usize = 50;
const WINDOWS: usize = 20;
const SECOND: u64 = 1_000_000_000;

fn procs() -> Result<(), String> {
    let initrd = Owned(fs::volume_open("Initrd").map_err(|e| format!("Initrd: {}", e.name()))?);
    let file = Owned(
        fs::open(initrd.0, "bin/true", open::READ).map_err(|e| format!("true: {}", e.name()))?,
    );
    let mut running = Vec::new();
    for i in 0..PROCS {
        let p = process::spawn(file.0, &["true"], [Handle::NONE; 3])
            .map_err(|e| format!("spawn {i}: {}", e.name()))?;
        running.push(Owned(p));
    }
    for (i, p) in running.iter().enumerate() {
        match process::wait(p.0, 30 * SECOND) {
            Ok(0) => {}
            Ok(code) => return Err(format!("process {i} exited with {code}")),
            Err(e) => return Err(format!("wait {i}: {}", e.name())),
        }
    }
    Ok(())
}

fn files() -> Result<(), String> {
    let data = Owned(fs::volume_open("Data").map_err(|e| format!("Data: {}", e.name()))?);
    let rw = open::READ | open::WRITE;
    let dir = Owned(
        fs::open(data.0, "stress", rw | open::CREATE | open::DIRECTORY)
            .map_err(|e| format!("Data:/stress: {}", e.name()))?,
    );
    let body = [b's'; 100];
    for i in 0..FILES {
        let name = format!("f{i:04}");
        let f = Owned(
            fs::open(dir.0, &name, rw | open::CREATE | open::EXCLUSIVE)
                .map_err(|e| format!("{name}: {}", e.name()))?,
        );
        fs::write(f.0, &body, 0).map_err(|e| format!("{name}: {}", e.name()))?;
    }
    let listed = fs::entries(dir.0).map_err(|e| e.name())?;
    if listed.len() != FILES || listed.iter().any(|e| e.kind != kind::FILE || e.size != 100) {
        return Err(format!("{} entries listed", listed.len()));
    }
    for i in 0..FILES {
        let name = format!("f{i:04}");
        fs::remove(dir.0, &name).map_err(|e| format!("rm {name}: {}", e.name()))?;
    }
    drop(dir);
    fs::remove(data.0, "stress").map_err(|e| format!("rm stress: {}", e.name()))
}

/// The pipe's writer: 64 MiB of a counting pattern into the end `arg`.
fn writer(arg: u64) -> u64 {
    let end = Handle::from_raw(arg);
    let mut chunk = alloc::vec![0u8; CHUNK];
    for c in 0..PIPE_BYTES / CHUNK {
        for (i, b) in chunk.iter_mut().enumerate() {
            *b = (c + i) as u8;
        }
        if libstela::pipe::write_all(end, &chunk).is_err() {
            return 1;
        }
    }
    let _ = handle_close(end);
    0
}

/// Bytes of each thread's stack.
const STACK: usize = 64 * 1024;

/// The threads' stacks: on the heap, made once (a `static` one would sit
/// in the program file: `ThreadStack` starts with a function pointer).
type Stacks = &'static [ThreadStack<STACK>];

fn pipe(stacks: Stacks) -> Result<(), String> {
    let (read, write) = libstela::pipe::create().map_err(|e| format!("pipe: {}", e.name()))?;
    let read = Owned(read);
    let t = libstela::spawn(writer, write.raw(), &stacks[CONNECTIONS]).map_err(|e| e.name())?;
    let mut buf = alloc::vec![0u8; CHUNK];
    let mut got = 0usize;
    loop {
        let n = libstela::pipe::read(read.0, &mut buf, 30 * SECOND)
            .map_err(|e| format!("read at {got}: {}", e.name()))?;
        if n == 0 {
            break;
        }
        for (i, &b) in buf[..n].iter().enumerate() {
            let at = got + i;
            if b != (at / CHUNK + at % CHUNK) as u8 {
                return Err(format!("wrong byte at {at}"));
            }
        }
        got += n;
    }
    let code = t.join().map_err(|e| e.name())?;
    if got != PIPE_BYTES || code != 0 {
        return Err(format!("{got} bytes, writer {code}"));
    }
    Ok(())
}

static URL: SpinMutex<String> = SpinMutex::new(String::new());

/// One download: the body's length (0 on failure).
fn fetcher(_: u64) -> u64 {
    let url = URL.lock().clone();
    match libstela::net::http_get(&url, 60 * SECOND) {
        Ok(r) if r.status == 200 => r.body.len() as u64,
        _ => 0,
    }
}

fn tcp(url: &str, stacks: Stacks) -> Result<(), String> {
    *URL.lock() = String::from(url);
    let mut threads = Vec::new();
    for stack in &stacks[..CONNECTIONS] {
        threads.push(libstela::spawn(fetcher, 0, stack).map_err(|e| e.name())?);
    }
    let lengths: Vec<u64> = threads.into_iter().map(|t| t.join().unwrap_or(0)).collect();
    if lengths.iter().any(|&n| n == 0 || n != lengths[0]) {
        return Err(format!("lengths {lengths:?}"));
    }
    Ok(())
}

fn windows() -> Result<(), String> {
    use toolbox::draw::Rect;
    let mut conn = toolbox::window::Connection::open().map_err(|e| e.name())?;
    let mut list = Vec::new();
    for i in 0..WINDOWS {
        let (x, y) = (40 + 30 * i as i32, 60 + 20 * i as i32);
        let w = conn
            .create_window(
                200,
                120,
                abi::window::flags::DOCUMENT,
                x,
                y,
                &format!("Stress {i}"),
            )
            .map_err(|e| format!("window {i}: {}", e.name()))?;
        list.push(w);
    }
    for frame in 0..20u32 {
        for (i, w) in list.iter_mut().enumerate() {
            let b = w.bounds();
            let shade = 0x10_1010 * ((frame + i as u32) % 15);
            w.canvas().fill(b, 0xF2_E8D5 ^ shade);
            w.canvas()
                .fill(Rect::new(frame as i32 * 8, 10, 30, 30), 0x1A_1410);
            conn.damage_all(w).map_err(|e| e.name())?;
        }
        // Let the server catch up; events are dropped.
        while conn.next_event(1_000_000).is_some() {}
    }
    for w in list {
        conn.close(w).map_err(|e| e.name())?;
    }
    Ok(())
}

/// What must come back: free frames, kernel heap in use, the highest
/// handle slot this program can get, running processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Levels {
    free_frames: u64,
    heap_used: u64,
    handle_slots: u32,
    running: usize,
}

fn levels() -> Result<Levels, String> {
    let m = process::memory().map_err(|e| e.name())?;
    // The handle table only grows when no slot is free: 600 copies at
    // once (more than any part holds) reach the same highest slot unless
    // handles were lost.
    let mut copies = Vec::new();
    for _ in 0..600 {
        copies.push(Owned(
            handle_duplicate(bootstrap::CONSOLE, Rights::SEND).map_err(|e| e.name())?,
        ));
    }
    let handle_slots = copies.iter().map(|h| h.0.index()).max().unwrap_or(0);
    drop(copies);
    let running = process::list()
        .map_err(|e| e.name())?
        .iter()
        .filter(|p| p.state == 1)
        .count();
    Ok(Levels {
        free_frames: m.free_frames,
        heap_used: m.heap_used,
        handle_slots,
        running,
    })
}

/// One part of a round.
type Part<'a> = &'a dyn Fn() -> Result<(), String>;

fn round(n: usize, url: Option<&str>, stacks: Stacks) -> Result<(), String> {
    let parts: [(&str, Part); 5] = [
        ("procs", &procs),
        ("files", &files),
        ("pipe", &|| pipe(stacks)),
        ("tcp", &|| url.map_or(Ok(()), |u| tcp(u, stacks))),
        ("windows", &windows),
    ];
    for (name, part) in parts {
        let before = process::memory().map_err(|e| e.name())?;
        let (t0, _) = process::uptime().unwrap_or((0, 0));
        part().map_err(|e| format!("{name}: {e}"))?;
        let (t1, _) = process::uptime().unwrap_or((0, 0));
        // What a part leaves behind, after the warm-up round: measured
        // once closed connections have left TIME_WAIT (2 s).
        let change = if n > 1 {
            libstela::sleep_ms(SETTLE_MS);
            let after = process::memory().map_err(|e| e.name())?;
            format!(
                "; {:+} frames, {:+} heap bytes",
                before.free_frames as i64 - after.free_frames as i64,
                after.heap_used as i64 - before.heap_used as i64
            )
        } else {
            String::new()
        };
        println!(
            "stress: round {n} {name} OK ({} ms{change})",
            (t1 - t0) / 1_000_000
        );
    }
    Ok(())
}

/// How long the system gets to settle before a measurement.
const SETTLE_MS: u64 = 2500;

/// Rounds: the first two warm up, the last is compared with the one before.
const ROUNDS: usize = 3;

fn main() -> i64 {
    let args = process::args();
    let url = args.get(1).map(String::as_str);
    let stacks: Stacks = Vec::leak((0..=CONNECTIONS).map(|_| ThreadStack::new()).collect());
    let mut after = Vec::new();
    for n in 1..=ROUNDS {
        if let Err(e) = round(n, url, stacks) {
            println!("stress: FAILED: {e}");
            return 1;
        }
        // The threads and processes are gone a moment after they end.
        libstela::sleep_ms(SETTLE_MS);
        match levels() {
            Ok(l) => {
                println!("stress: after round {n}: {l:?}");
                after.push(l);
            }
            Err(e) => {
                println!("stress: FAILED: {e}");
                return 1;
            }
        }
    }
    let (a, b) = (after[ROUNDS - 2], after[ROUNDS - 1]);
    // Small slack for what the kernel keeps on its own (timers, the
    // network's ARP table, a block cache that settles).
    let frames = a.free_frames.abs_diff(b.free_frames);
    let heap = a.heap_used.abs_diff(b.heap_used);
    if frames > 64 || heap > 64 * 1024 || b.handle_slots > a.handle_slots || b.running != a.running
    {
        println!("stress: LEAK: {frames} frames, {heap} heap bytes, handles {a:?} -> {b:?}");
        return 1;
    }
    println!("stress: OK ({frames} frames, {heap} heap bytes apart)");
    0
}
