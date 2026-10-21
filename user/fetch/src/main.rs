// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! fetch: `fetch http://host[:port]/path [file]` gets a URL over HTTP/1.0
//! and prints the body (or writes it to `file`, relative to the working
//! folder).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

use alloc::string::String;
use libstela::fs::{self, open};
use libstela::ipc::handle_close;
use libstela::net;
use libstela::{bootstrap, print, println, process};

extern crate alloc;

libstela::entry!(main);

fn main() -> i64 {
    let mut args = process::args();
    // --ca FILE: trust the root certificates in FILE (DER, one after
    // another) for https:// instead of the system's (P12, docs/tls.md).
    let mut roots = None;
    if args.get(1).map(String::as_str) == Some("--ca") && args.len() > 2 {
        let path = args.remove(2);
        args.remove(1);
        let data = if path.contains(':') {
            fs::read_path(&path)
        } else {
            fs::open(bootstrap::CWD, &path, open::READ).and_then(|h| {
                let data = fs::read_all(h);
                let _ = handle_close(h);
                data
            })
        };
        match data {
            Ok(d) => roots = Some(libstela::tls::Roots::from_der(d)),
            Err(e) => {
                println!("fetch: {path}: {}", e.name());
                return 1;
            }
        }
    }
    let Some(url) = args.get(1) else {
        println!("fetch: usage: fetch [--ca roots.der] http(s)://host[:port]/path [file]");
        return 2;
    };
    if net::wait_configured(5_000_000_000).is_none() {
        println!("fetch: the network is not up");
        return 1;
    }
    let response = match net::http_get_raw_with(url, 10_000_000_000, roots.as_ref())
        .and_then(|raw| net::parse_response(&raw).ok_or(libstela::abi::Error::IoError))
    {
        Ok(r) => r,
        Err(e) => {
            println!("fetch: {url}: {}", e.name());
            return 1;
        }
    };
    println!(
        "fetch: {url}: HTTP {}, {} bytes",
        response.status,
        response.body.len()
    );
    match args.get(2) {
        Some(file) => {
            let h = match fs::open(
                bootstrap::CWD,
                file,
                open::READ | open::WRITE | open::CREATE | open::TRUNCATE,
            ) {
                Ok(h) => h,
                Err(e) => {
                    println!("fetch: {file}: {}", e.name());
                    return 1;
                }
            };
            let r = fs::write(h, &response.body, 0);
            let _ = handle_close(h);
            if let Err(e) = r {
                println!("fetch: {file}: {}", e.name());
                return 1;
            }
            println!("fetch: saved {file}");
        }
        None => {
            for chunk in response.body.utf8_chunks() {
                print!("{}", chunk.valid());
            }
            if !response.body.ends_with(b"\n") {
                println!();
            }
        }
    }
    if response.status == 200 { 0 } else { 1 }
}
