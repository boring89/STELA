// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! ping: sends ICMP echo requests (`ping host [count]`, default 4, one a
//! second) and prints the replies with their round-trip time. Exits 0 if
//! any reply came.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::net::{self, Socket, ip_text};
use libstela::{println, process};

libstela::entry!(main);

fn main() -> i64 {
    let args = process::args();
    let Some(host) = args.get(1) else {
        println!("ping: usage: ping host [count]");
        return 2;
    };
    let count: u16 = args.get(2).and_then(|c| c.parse().ok()).unwrap_or(4);
    if net::wait_configured(5_000_000_000).is_none() {
        println!("ping: the network is not up");
        return 1;
    }
    let ip = match net::resolve(host) {
        Ok(ip) => ip,
        Err(e) => {
            println!("ping: {host}: {}", e.name());
            return 1;
        }
    };
    let s = match Socket::icmp() {
        Ok(s) => s,
        Err(e) => {
            println!("ping: {}", e.name());
            return 1;
        }
    };
    println!("PING {host} ({})", ip_text(ip));
    let payload = *b"STELA ping 0123456789abcdefghijklmnopqrstuvwxyz";
    let mut replies = 0;
    let mut buf = [0u8; 128];
    for seq in 1..=count {
        let sent = libstela::now_ns();
        if let Err(e) = s.send(&payload, Some((ip, seq))) {
            println!("ping: {}", e.name());
            return 1;
        }
        let deadline = sent + 1_000_000_000;
        loop {
            let left = deadline.saturating_sub(libstela::now_ns());
            match s.recv(&mut buf, left) {
                Ok((n, from, got)) if got == seq && from == ip && buf[..n] == payload => {
                    let us = (libstela::now_ns() - sent) / 1000;
                    println!(
                        "{n} bytes from {}: seq={seq} time={}.{:03} ms",
                        ip_text(from),
                        us / 1000,
                        us % 1000
                    );
                    replies += 1;
                    break;
                }
                Ok(_) => {}
                Err(_) => {
                    println!("seq={seq}: no reply");
                    break;
                }
            }
        }
        let next = sent + 1_000_000_000;
        let now = libstela::now_ns();
        if seq < count && now < next {
            libstela::sleep_ns(next - now);
        }
    }
    println!("ping: {count} sent, {replies} received");
    if replies > 0 { 0 } else { 1 }
}
