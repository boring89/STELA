// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! ehfs-ls: lists a folder of an eHFS image (read only).
//!
//! Usage: `ehfs-ls [--offset SECTORS] IMAGE [EHFSPATH]`

use ehfs_tools::{ImageFile, describe, resolve, take_geometry};
use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (offset, sectors, rest) = take_geometry(args).unwrap_or_else(|e| {
        eprintln!("ehfs-ls: {e}");
        exit(2)
    });
    let (target, path) = match rest.as_slice() {
        [t] => (t.as_str(), "/"),
        [t, p] => (t.as_str(), p.as_str()),
        _ => {
            eprintln!("usage: ehfs-ls [--offset SECTORS] IMAGE [EHFSPATH]");
            exit(2)
        }
    };
    let image = ImageFile::open(target, false, offset, sectors).unwrap_or_else(|e| {
        eprintln!("ehfs-ls: {e}");
        exit(2)
    });
    let mut v = ehfs::Volume::mount(&image, true).unwrap_or_else(|e| {
        eprintln!("ehfs-ls: {target}: {}", describe(e));
        exit(2)
    });
    let dir = resolve(&mut v, path).unwrap_or_else(|e| {
        eprintln!("ehfs-ls: {path}: {}", describe(e));
        exit(1)
    });
    if !dir.is_dir {
        println!("{:>12}  {}", dir.size(), String::from_utf8_lossy(&dir.name));
        return;
    }
    let entries = v.list(dir.cnid).unwrap_or_else(|e| {
        eprintln!("ehfs-ls: {path}: {}", describe(e));
        exit(1)
    });
    for e in entries {
        let name = String::from_utf8_lossy(&e.name);
        if e.is_dir {
            println!("{:>12}  {name}/", format!("({} items)", e.valence));
        } else {
            println!("{:>12}  {name}", e.size());
        }
    }
}
