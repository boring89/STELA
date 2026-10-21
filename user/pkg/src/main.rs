// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! `pkg`: installs programs from the package server (P12,
//! `docs/pkg.md`).
//!
//! ```text
//! pkg update               fetch the signed index (Data:/.stela/pkg.conf)
//! pkg list                 what is installed
//! pkg search [WORD]        what the index has
//! pkg install NAME...      with what they need; or pkg install file.tar
//! pkg remove NAME
//! pkg upgrade              install the newer versions
//! ```

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use libstela::{println, process};
use pkgcore::ops;

libstela::entry!(main);

fn usage() -> i64 {
    println!("usage: pkg update | list | search [word] | install name... | remove name | upgrade");
    2
}

fn fail(f: ops::Failure) -> i64 {
    println!("pkg: {}", f.text());
    1
}

fn main() -> i64 {
    let args = process::args();
    let words: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let Some((&command, rest)) = words.split_first() else {
        return usage();
    };
    let needs_network = matches!(command, "update" | "upgrade")
        || (command == "install" && rest.iter().any(|n| !n.ends_with(".tar")));
    if needs_network && libstela::net::wait_configured(5_000_000_000).is_none() {
        println!("pkg: the network is not up");
        return 1;
    }
    match command {
        "update" => match ops::update() {
            Ok(n) => {
                println!("pkg: the index lists {n} packages");
                0
            }
            Err(f) => fail(f),
        },
        "list" => {
            let list = ops::installed();
            if list.is_empty() {
                println!("pkg: nothing is installed");
            }
            for (name, version) in list {
                println!("{name} {version}");
            }
            0
        }
        "search" => match ops::search(rest.first().copied().unwrap_or("")) {
            Ok(found) => {
                for p in found {
                    println!(
                        "{} {} - {} ({})",
                        p.name, p.version, p.description, p.license
                    );
                }
                0
            }
            Err(f) => fail(f),
        },
        "install" if !rest.is_empty() => match ops::install(rest, |step| println!("pkg: {step}")) {
            Ok(done) => {
                for p in done {
                    println!("pkg: installed {} {}", p.name, p.version);
                }
                0
            }
            Err(f) => fail(f),
        },
        "remove" if rest.len() == 1 => match ops::remove(rest[0]) {
            Ok(()) => {
                println!("pkg: removed {}", rest[0]);
                0
            }
            Err(f) => fail(f),
        },
        "upgrade" => {
            if let Err(f) = ops::update() {
                return fail(f);
            }
            let names = match ops::upgradable() {
                Ok(n) => n,
                Err(f) => return fail(f),
            };
            if names.is_empty() {
                println!("pkg: everything is up to date");
                return 0;
            }
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            match ops::install(&refs, |step| println!("pkg: {step}")) {
                Ok(done) => {
                    for p in done {
                        println!("pkg: upgraded {} to {}", p.name, p.version);
                    }
                    0
                }
                Err(f) => fail(f),
            }
        }
        _ => usage(),
    }
}
