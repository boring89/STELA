// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! eHFS, STELA's file system (`docs/ehfs.md`): the on-disk format of
//! 68k_HAL's eHFS version 1, read and written so that a volume made by
//! either system works on the other. 68k_HAL's `docs/ehfs.md` is the
//! authority for the format; this is a new implementation in Rust.
//!
//! - 4 KiB blocks, 64-bit block numbers, an allocation bitmap, extents.
//! - Catalog, extents and attributes B+-trees in 16 KiB nodes, CRC-32C
//!   on every node and the superblock.
//! - A metadata journal (one transaction at a time, ordered data).
//! - Names: UTF-8, case kept but ignored (NFC + simple case folding,
//!   Unicode 15.0), persistent 64-bit CNIDs, data and resource forks,
//!   Finder information, extended attributes.
//!
//! Pure logic over a [`block::BlockDevice`]; `no_std` with `alloc`,
//! tested on the host with in-memory disks.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod btree;
mod catalog;
mod check;
pub mod crc32c;
mod file;
mod fork;
pub mod format;
mod mkfs;
pub mod name;
#[allow(missing_docs, clippy::unreadable_literal)]
mod unicode_data;
mod volume;

pub use catalog::Entry;
pub use check::Report;
pub use file::{ATTR_NAME_MAX, ATTR_VALUE_MAX};
pub use format::cnid::ROOT;
pub use mkfs::{FormatOptions, MIN_BLOCKS, format};
pub use volume::Volume;

/// Why an eHFS operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The device failed.
    Io,
    /// Something on the disk is not what eHFS writes.
    Corrupt,
    /// No free blocks (or B-tree nodes, or CNIDs).
    NoSpace,
    /// A bad argument.
    Invalid,
    /// Mounted read-only.
    ReadOnly,
    /// The transaction would be too large.
    TxnTooLarge,
    /// No such item.
    NotFound,
    /// The name is taken.
    Exists,
    /// Not a folder.
    NotDir,
    /// A folder where a file was expected.
    IsDir,
    /// The folder is not empty.
    NotEmpty,
    /// Not a valid name.
    BadName,
    /// A feature this code does not know (incompat flags, block size).
    Unsupported,
}

/// Whether `bytes` (block 0 of a device) holds an eHFS superblock.
pub fn is_ehfs(block0: &[u8]) -> bool {
    block0.len() >= format::SB_OFFSET + 8
        && &block0[format::SB_OFFSET..format::SB_OFFSET + 8] == format::MAGIC
}

#[cfg(test)]
mod tests;
