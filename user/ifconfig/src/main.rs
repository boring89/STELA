// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! ifconfig: the network card and the interface's address
//! (`docs/net.md`). Waits up to 5 seconds for DHCP if it is not done yet.
//! `ifconfig resolve NAME` looks a name up through DNS instead.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use libstela::net::{self, ip_text};
use libstela::{println, process};

libstela::entry!(main);

fn main() -> i64 {
    let args = process::args();
    if let (Some("resolve"), Some(name)) = (args.get(1).map(|a| a.as_str()), args.get(2)) {
        if net::wait_configured(5_000_000_000).is_none() {
            println!("ifconfig: the network is not up");
            return 1;
        }
        return match net::resolve(name) {
            Ok(ip) => {
                println!("{name} has address {}", ip_text(ip));
                0
            }
            Err(e) => {
                println!("ifconfig: {name}: {}", e.name());
                1
            }
        };
    }
    let Some(mut i) = net::info() else {
        println!("ifconfig: no network card");
        return 1;
    };
    if !i.configured
        && let Some(c) = net::wait_configured(5_000_000_000)
    {
        i = c;
    }
    let m = i.mac;
    println!(
        "{}: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, link {}",
        i.driver_name(),
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5],
        if i.link { "up" } else { "down" }
    );
    if !i.configured {
        println!("  no address (DHCP has not answered)");
        return 1;
    }
    println!(
        "  inet {} mask {} gateway {} dns {}",
        ip_text(i.ip),
        ip_text(i.mask),
        ip_text(i.gateway),
        ip_text(i.dns)
    );
    0
}
