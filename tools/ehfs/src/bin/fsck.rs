// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! fsck.ehfs: checks an eHFS volume in an image file (it does not
//! repair). Exit status: 0 clean, 1 problems, 2 unreadable.
//!
//! Usage: `fsck.ehfs [--offset SECTORS] [--sectors N] IMAGE`

use ehfs_tools::{ImageFile, describe, take_geometry};
use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (offset, sectors, rest) = take_geometry(args).unwrap_or_else(|e| {
        eprintln!("fsck.ehfs: {e}");
        exit(2)
    });
    let [target] = rest.as_slice() else {
        eprintln!("usage: fsck.ehfs [--offset SECTORS] [--sectors N] IMAGE");
        exit(2)
    };
    let image = ImageFile::open(target, false, offset, sectors).unwrap_or_else(|e| {
        eprintln!("fsck.ehfs: {e}");
        exit(2)
    });
    // Read-only: a committed transaction left in the journal is read
    // through, not written home.
    let mut v = ehfs::Volume::mount(&image, true).unwrap_or_else(|e| {
        eprintln!("fsck.ehfs: {target}: {}", describe(e));
        exit(2)
    });
    let report = v.check();
    let (files, folders) = v.counts();
    let state = match v.state() {
        ehfs::format::state::CLEAN => "unmounted cleanly",
        ehfs::format::state::MOUNTED => "not unmounted cleanly",
        _ => "marked with errors",
    };
    if report.clean() {
        println!(
            "fsck.ehfs: {target}: \"{}\" is clean ({files} files, {folders} folders, {} of {} blocks free; {state})",
            v.name(),
            v.free_blocks(),
            v.block_count()
        );
    } else {
        println!(
            "fsck.ehfs: {target}: {} problem(s); first: {}",
            report.errors, report.first
        );
        exit(1);
    }
}
