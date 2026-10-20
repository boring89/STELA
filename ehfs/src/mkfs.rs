// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Making an empty volume, laid out as 68k_HAL's mkfs does:
//!
//! ```text
//!  0        superblock (bytes 1024..4095)
//!  1 ..     allocation bitmap
//!  ..       journal (1/256 of the volume, 260..16384 blocks)
//!  ..       catalog (32 nodes), extents (8) and attributes (8) B-trees
//!  ..       free
//!  last     copy of the superblock
//! ```
//!
//! Written straight to the device (a crash during mkfs leaves no volume);
//! the root folder is then made through the journal like any change.

use crate::Error;
use crate::btree::tree_header;
use crate::crc32c::crc32c;
use crate::format::*;
use crate::name;
use crate::volume::{Volume, sb_seal};
use alloc::vec;
use block::BlockDevice;

/// The smallest volume: 1024 blocks (4 MiB).
pub const MIN_BLOCKS: u64 = 1024;
const CATALOG_NODES: u64 = 32;
const EXTENTS_NODES: u64 = 8;
const ATTRIBUTE_NODES: u64 = 8;

/// What a new volume gets.
#[derive(Clone, Debug)]
pub struct FormatOptions<'a> {
    /// The volume name (UTF-8, a valid eHFS name).
    pub name: &'a str,
    /// The volume UUID.
    pub uuid: [u8; 16],
    /// Nanoseconds since 1970 (UTC).
    pub now: u64,
    /// Journal blocks (0: the default).
    pub journal_blocks: u64,
}

fn set_fork(desc: &mut [u8], start: u64, count: u64) {
    desc.fill(0);
    put64(desc, fork::LENGTH, count * BLOCK_SIZE as u64);
    put64(desc, fork::BLOCKS, count);
    put64(desc, fork::EXTENT, start);
    put64(desc, fork::EXTENT + 8, count);
}

/// Makes an empty eHFS volume on all of `dev` and returns it unmounted
/// (clean).
pub fn format<D: BlockDevice>(dev: D, options: &FormatOptions<'_>) -> Result<(), Error> {
    let blocks = dev.block_count();
    if dev.read_only() {
        return Err(Error::ReadOnly);
    }
    if blocks < MIN_BLOCKS {
        return Err(Error::Invalid);
    }
    let name = options.name.as_bytes();
    if !name::valid(name) {
        return Err(Error::BadName);
    }
    let bitmap = blocks.div_ceil((BLOCK_SIZE * 8) as u64);
    let mut journal = match options.journal_blocks {
        0 => (blocks / 256).min(16384),
        n => n,
    };
    journal = journal.max(TXN_BLOCKS as u64 + 4);
    let catalog_start = 1 + bitmap + journal;
    let extents_start = catalog_start + CATALOG_NODES * NODE_BLOCKS;
    let attributes_start = extents_start + EXTENTS_NODES * NODE_BLOCKS;
    let used_end = attributes_start + ATTRIBUTE_NODES * NODE_BLOCKS;
    if used_end + 1 >= blocks {
        return Err(Error::NoSpace);
    }
    let mut v = Volume::raw(dev, blocks);
    {
        let sb = &mut v.sb;
        sb[..8].copy_from_slice(MAGIC);
        put32(sb, sb::VERSION, VERSION);
        put32(sb, sb::BLOCK_SIZE, BLOCK_SIZE as u32);
        put64(sb, sb::BLOCK_COUNT, blocks);
        put64(sb, sb::FREE_BLOCKS, blocks - used_end - 1);
        put64(sb, sb::NEXT_CNID, cnid::FIRST);
        put32(sb, sb::STATE, state::CLEAN);
        sb[sb::UUID..sb::UUID + 16].copy_from_slice(&options.uuid);
        put64(sb, sb::CREATED, options.now);
        put64(sb, sb::MODIFIED, options.now);
        put64(sb, sb::CHECKED, options.now);
        let fork_at = |s: Special| sb::FORKS + s as usize * FORK_SIZE;
        for (s, start, count) in [
            (Special::Bitmap, 1, bitmap),
            (Special::Journal, 1 + bitmap, journal),
            (Special::Catalog, catalog_start, CATALOG_NODES * NODE_BLOCKS),
            (Special::Extents, extents_start, EXTENTS_NODES * NODE_BLOCKS),
            (
                Special::Attributes,
                attributes_start,
                ATTRIBUTE_NODES * NODE_BLOCKS,
            ),
        ] {
            let at = fork_at(s);
            set_fork(&mut sb[at..at + FORK_SIZE], start, count);
        }
        sb[sb::NAME] = name.len() as u8;
        sb[sb::NAME + 1..sb::NAME + 1 + name.len()].copy_from_slice(name);
        put32(sb, sb::UNICODE, UNICODE_VERSION);
        put32(sb, sb::NODE_SIZE, NODE_SIZE as u32);
    }
    // The bitmap: 0 .. used_end and the last block are in use.
    let mut b = vec![0u8; BLOCK_SIZE];
    for i in 0..bitmap {
        let first = i * (BLOCK_SIZE * 8) as u64;
        b.fill(0);
        for n in first..(first + (BLOCK_SIZE * 8) as u64).min(blocks) {
            if n < used_end || n == blocks - 1 {
                b[((n - first) / 8) as usize] |= 1 << (n % 8);
            }
        }
        v.dev_write(1 + i, &b)?;
    }
    // The journal: header (no transaction yet), an empty descriptor.
    b.fill(0);
    b[..8].copy_from_slice(journal::MAGIC);
    let crc = crc32c(0, &b[..journal::HDR_CRC]);
    put32(&mut b, journal::HDR_CRC, crc);
    v.dev_write(1 + bitmap, &b)?;
    b.fill(0);
    v.dev_write(2 + bitmap, &b)?;
    // Each tree: its header node.
    for (start, nodes) in [
        (catalog_start, CATALOG_NODES),
        (extents_start, EXTENTS_NODES),
        (attributes_start, ATTRIBUTE_NODES),
    ] {
        let header = tree_header(nodes);
        for k in 0..NODE_BLOCKS {
            let part = &header[k as usize * BLOCK_SIZE..(k as usize + 1) * BLOCK_SIZE];
            v.dev_write(start + k, part)?;
        }
    }
    // The superblock and its copy.
    let mut sbb = vec![0u8; BLOCK_SIZE];
    sbb[SB_OFFSET..].copy_from_slice(&v.sb);
    sb_seal(&mut sbb[SB_OFFSET..]);
    v.dev_write(blocks - 1, &sbb)?;
    v.dev_write(0, &sbb)?;
    // The root folder, through the journal.
    let mut v = Volume::mount(v.dev, false)?;
    v.begin()?;
    let r = v.cat_make_root(name, options.now);
    v.finish(r)?;
    v.unmount()
}
