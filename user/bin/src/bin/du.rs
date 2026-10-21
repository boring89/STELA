// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! du: the bytes of the files under each folder named (default `.`),
//! in KiB.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use cli::sys;
use libstela::abi::kind;
use libstela::fs::{self, open};
use libstela::println;

libstela::entry!(main);

fn total(dir: &sys::Owned, depth: usize) -> u64 {
    let Ok(entries) = fs::entries(dir.0) else {
        return 0;
    };
    let mut sum = 0;
    for e in entries {
        if e.kind == kind::DIRECTORY {
            if depth < 64
                && let Ok(sub) = fs::open(dir.0, e.name(), open::READ)
            {
                sum += total(&sys::Owned(sub), depth + 1);
            }
        } else {
            sum += e.size;
        }
    }
    sum
}

fn main() -> i64 {
    let mut args = sys::args();
    if args.is_empty() {
        args = vec![String::from(".")];
    }
    let mut code = 0;
    for a in &args {
        match sys::open_path(a, open::READ) {
            Ok((dir, loc)) => {
                let bytes = match fs::stat(dir.0) {
                    Ok(st) if st.kind == kind::FILE => st.size,
                    _ => total(&dir, 0),
                };
                println!("{:8} KiB  {loc}", bytes.div_ceil(1024));
            }
            Err(e) => code = sys::fail("du", &e),
        }
    }
    code
}
