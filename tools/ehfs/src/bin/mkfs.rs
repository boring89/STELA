// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! mkfs.ehfs: makes an empty eHFS volume in an image file.
//!
//! Usage: `mkfs.ehfs [-L name] [--size MiB] [--offset SECTORS] [--sectors N]
//! [--journal BLOCKS] IMAGE`
//!
//! With `--size`, a missing image is created (or a short one extended).
//! Only regular files are accepted.

use ehfs_tools::{ImageFile, describe, now, take_geometry};
use std::process::exit;

fn usage() -> ! {
    eprintln!(
        "usage: mkfs.ehfs [-L name] [--size MiB] [--offset SECTORS] [--sectors N] [--journal BLOCKS] IMAGE"
    );
    exit(2)
}

fn uuid() -> [u8; 16] {
    // From /dev/urandom; the time as a fallback.
    let mut u = [0u8; 16];
    if let Ok(bytes) = std::fs::read("/proc/sys/kernel/random/uuid") {
        let hex: Vec<u8> = bytes
            .iter()
            .copied()
            .filter(u8::is_ascii_hexdigit)
            .collect();
        for (i, pair) in hex.chunks(2).take(16).enumerate() {
            u[i] = u8::from_str_radix(std::str::from_utf8(pair).unwrap_or("0"), 16).unwrap_or(0);
        }
    } else {
        u[..8].copy_from_slice(&now().to_le_bytes());
    }
    u
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (offset, sectors, rest) = take_geometry(args).unwrap_or_else(|e| {
        eprintln!("mkfs.ehfs: {e}");
        usage()
    });
    let mut name = String::from("Data");
    let mut size_mib = None;
    let mut journal = 0;
    let mut target = None;
    let mut it = rest.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-L" => name = it.next().unwrap_or_else(|| usage()),
            "--size" => size_mib = it.next().and_then(|v| v.parse::<u64>().ok()),
            "--journal" => {
                journal = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            _ if target.is_none() && !a.starts_with('-') => target = Some(a),
            _ => usage(),
        }
    }
    let Some(target) = target else { usage() };
    if let Some(mib) = size_mib {
        let exists = std::fs::metadata(&target);
        if exists.as_ref().is_ok_and(|m| !m.is_file()) {
            eprintln!("mkfs.ehfs: {target}: not a regular file; refused");
            exit(2);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&target)
            .unwrap_or_else(|e| {
                eprintln!("mkfs.ehfs: {target}: {e}");
                exit(2)
            });
        let want = mib << 20;
        if file.metadata().map_or(0, |m| m.len()) < want && file.set_len(want).is_err() {
            eprintln!("mkfs.ehfs: {target}: cannot set the size");
            exit(2);
        }
    }
    let image = ImageFile::open(&target, true, offset, sectors).unwrap_or_else(|e| {
        eprintln!("mkfs.ehfs: {e}");
        exit(2)
    });
    let blocks = block::BlockDevice::block_count(&image);
    let options = ehfs::FormatOptions {
        name: &name,
        uuid: uuid(),
        now: now(),
        journal_blocks: journal,
    };
    if let Err(e) = ehfs::format(&image, &options) {
        eprintln!("mkfs.ehfs: {target}: {}", describe(e));
        exit(1);
    }
    println!("mkfs.ehfs: {target}: \"{name}\", {blocks} blocks of 4 KiB");
}
