// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A Rust program with the standard library on STELA's Linux personality
//! (P12, `docs/linux.md`): threads, a mutex, a channel, files, time and
//! environment. Built for `x86_64-unknown-linux-musl` (static, not PIE).

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("hello-std: {} args, first {:?}", args.len(), args.get(1));

    // Threads adding up under a mutex.
    let total = Arc::new(Mutex::new(0u64));
    let workers: Vec<_> = (1..=8u64)
        .map(|n| {
            let total = Arc::clone(&total);
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    *total.lock().unwrap() += n;
                }
                n * 2
            })
        })
        .collect();
    let returned: u64 = workers.into_iter().map(|w| w.join().unwrap()).sum();
    let sum = *total.lock().unwrap();
    assert_eq!(sum, 36_000);
    assert_eq!(returned, 72);
    println!("hello-std: 8 threads added up to {sum}");

    // A channel between threads.
    let (tx, rx) = mpsc::channel();
    let sender = std::thread::spawn(move || {
        for i in 0..5 {
            tx.send(i * i).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    let squares: Vec<i32> = rx.iter().collect();
    sender.join().unwrap();
    assert_eq!(squares, [0, 1, 4, 9, 16]);
    println!("hello-std: channel {squares:?}");

    // Files.
    let path = "/tmp/hello-std.txt";
    std::fs::write(path, "written by Rust\n").unwrap();
    let back = std::fs::read_to_string(path).unwrap();
    assert_eq!(back, "written by Rust\n");
    std::fs::remove_file(path).unwrap();
    assert!(std::fs::metadata(path).is_err());

    // Time.
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(20));
    assert!(start.elapsed() >= Duration::from_millis(20));
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(since > 1_700_000_000);

    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    println!(
        "hello-std: OK ({cpus} cpu, HOME={})",
        std::env::var("HOME").unwrap_or_default()
    );
}
