// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! stat: what the file system knows about each item named.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use cli::sys;
use libstela::abi::kind;
use libstela::fs::{self, open};
use libstela::println;

libstela::entry!(main);

fn main() -> i64 {
    let args = sys::args();
    if args.is_empty() {
        return sys::fail("stat", "usage: stat item...");
    }
    let mut code = 0;
    for a in &args {
        let st = sys::open_path(a, open::READ).and_then(|(h, loc)| {
            fs::stat(h.0)
                .map(|st| (st, loc))
                .map_err(|e| alloc::string::String::from(sys::describe(e)))
        });
        match st {
            Ok((st, loc)) => {
                println!("{loc}");
                println!(
                    "  {}, {} bytes, id {}{}",
                    if st.kind == kind::DIRECTORY {
                        "folder"
                    } else {
                        "file"
                    },
                    st.size,
                    st.id,
                    if st.flags & 1 != 0 { ", read only" } else { "" }
                );
                println!("  made     {} UTC", cli::date(st.ctime));
                println!("  changed  {} UTC", cli::date(st.mtime));
            }
            Err(e) => code = sys::fail("stat", &e),
        }
    }
    code
}
