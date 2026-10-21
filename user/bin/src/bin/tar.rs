// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! tar: ustar archives. `tar t ARCHIVE` lists the members; `tar x
//! ARCHIVE [FOLDER]` makes them in FOLDER (default `.`).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use cli::sys;
use libstela::fs::{self, open};
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    let args = sys::args();
    let (Some(mode), Some(archive)) = (args.first(), args.get(1)) else {
        return sys::fail("tar", "usage: tar t archive | tar x archive [folder]");
    };
    let data = match sys::read_file(archive) {
        Ok(d) => d,
        Err(e) => return sys::fail("tar", &e),
    };
    let entries = match cli::tar_entries(&data) {
        Ok(e) => e,
        Err(e) => return sys::fail("tar", &e),
    };
    match mode.as_str() {
        "t" => {
            for e in &entries {
                if e.is_dir {
                    println!("{}/", e.name);
                } else {
                    println!("{:>10}  {}", e.size, e.name);
                }
            }
            0
        }
        "x" => {
            let to = args.get(2).map_or(".", String::as_str);
            let (dir, _) = match sys::open_path(to, open::READ) {
                Ok(d) => d,
                Err(e) => return sys::fail("tar", &e),
            };
            let mut code = 0;
            for e in &entries {
                if e.name.split('/').any(|p| p == ".." || p.is_empty()) {
                    code = sys::fail(
                        "tar",
                        &alloc::format!("{}: skipped (not below the folder)", e.name),
                    );
                    continue;
                }
                let flags = if e.is_dir {
                    open::READ | open::CREATE | open::DIRECTORY
                } else {
                    open::READ | open::WRITE | open::CREATE | open::TRUNCATE
                };
                match fs::open(dir.0, &e.name, flags) {
                    Ok(h) => {
                        let h = sys::Owned(h);
                        if !e.is_dir {
                            let end = (e.data + e.size as usize).min(data.len());
                            if let Err(err) = sys::write_all(h.0, &data[e.data..end], 0) {
                                code = sys::fail(
                                    "tar",
                                    &alloc::format!("{}: {}", e.name, sys::describe(err)),
                                );
                                continue;
                            }
                        }
                        println!("{}", e.name);
                    }
                    Err(err) => {
                        code =
                            sys::fail("tar", &alloc::format!("{}: {}", e.name, sys::describe(err)));
                    }
                }
            }
            code
        }
        _ => sys::fail("tar", "usage: tar t archive | tar x archive [folder]"),
    }
}
