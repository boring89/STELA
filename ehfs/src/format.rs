// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The on-disk format of eHFS version 1, exactly as 68k_HAL defines it
//! (its `docs/ehfs.md` and `kernel/fs/ehfs/format.h` are the authority).
//! Little endian, except the integers inside B-tree keys, which are big
//! endian so that every tree orders its keys by plain byte comparison.

/// Block size (decision E6: only 4 KiB).
pub const BLOCK_SIZE: usize = 4096;

/// The superblock: 3072 bytes at byte 1024 of block 0; a copy at byte
/// 1024 of the last block.
pub const SB_OFFSET: usize = 1024;
/// Superblock size.
pub const SB_SIZE: usize = 3072;
/// `"eHFS\0\r\n\x1a"`.
pub const MAGIC: &[u8; 8] = b"eHFS\0\r\n\x1a";
/// Format version.
pub const VERSION: u32 = 1;

/// Superblock field offsets.
pub mod sb {
    #![allow(missing_docs)]
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 8;
    pub const BLOCK_SIZE: usize = 12;
    pub const BLOCK_COUNT: usize = 16;
    pub const FREE_BLOCKS: usize = 24;
    pub const NEXT_CNID: usize = 32;
    pub const FILE_COUNT: usize = 40;
    pub const FOLDER_COUNT: usize = 48;
    pub const COMPAT: usize = 56;
    pub const RO_COMPAT: usize = 60;
    pub const INCOMPAT: usize = 64;
    pub const STATE: usize = 68;
    pub const UUID: usize = 72;
    pub const CREATED: usize = 88;
    pub const MODIFIED: usize = 96;
    pub const CHECKED: usize = 104;
    pub const MOUNTED: usize = 112;
    /// Six fork descriptors of the special files.
    pub const FORKS: usize = 120;
    /// Length byte, then up to 255 bytes of UTF-8.
    pub const NAME: usize = 984;
    /// Unicode version of the name tables (decision E2), 0x0F0000.
    pub const UNICODE: usize = 1240;
    /// B-tree node size.
    pub const NODE_SIZE: usize = 1244;
    /// CRC-32C of bytes 0..3068.
    pub const CRC: usize = 3068;
}

/// Volume states.
pub mod state {
    #![allow(missing_docs)]
    pub const CLEAN: u32 = 0;
    /// Mounted for writing (found at mount: not cleanly unmounted).
    pub const MOUNTED: u32 = 1;
    pub const ERRORS: u32 = 2;
}

/// The special files, in the superblock's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Special {
    /// The allocation bitmap.
    Bitmap = 0,
    /// The catalog B-tree.
    Catalog = 1,
    /// The extents B-tree.
    Extents = 2,
    /// The attributes B-tree.
    Attributes = 3,
    /// The journal.
    Journal = 4,
    /// Bad blocks.
    BadBlocks = 5,
}

/// Number of special files.
pub const SPECIAL_FORKS: usize = 6;

/// Fork descriptor: 144 bytes: length (bytes), blocks, 8 extents of
/// (start u64, count u64).
pub const FORK_SIZE: usize = 144;
/// Extents kept in a fork descriptor.
pub const INLINE_EXTENTS: usize = 8;
/// Fork descriptor field offsets.
pub mod fork {
    #![allow(missing_docs)]
    pub const LENGTH: usize = 0;
    pub const BLOCKS: usize = 8;
    pub const EXTENT: usize = 16;
}

/// Reserved CNIDs.
pub mod cnid {
    #![allow(missing_docs)]
    pub const ROOT_PARENT: u64 = 1;
    pub const ROOT: u64 = 2;
    pub const EXTENTS: u64 = 3;
    pub const CATALOG: u64 = 4;
    pub const BITMAP: u64 = 5;
    pub const ATTRIBUTES: u64 = 6;
    pub const JOURNAL: u64 = 7;
    pub const BAD_BLOCKS: u64 = 8;
    /// The first CNID handed out.
    pub const FIRST: u64 = 16;
    /// The Mac sees 32-bit ids (decision E5): no CNID above this.
    pub const MAC_MAX: u64 = 0xFFFF_FFFF;
}

/// B-tree node size (decision E3): 16 KiB.
pub const NODE_SIZE: usize = 16384;
/// Blocks per node.
pub const NODE_BLOCKS: u64 = (NODE_SIZE / BLOCK_SIZE) as u64;

/// Node header offsets.
pub mod node {
    #![allow(missing_docs)]
    /// CRC-32C of bytes 4..end.
    pub const CRC: usize = 0;
    pub const KIND: usize = 4;
    /// 1 = leaf.
    pub const HEIGHT: usize = 5;
    pub const COUNT: usize = 6;
    pub const NEXT: usize = 8;
    pub const PREV: usize = 16;
    /// Where free space starts (after the record data).
    pub const FREE: usize = 24;
    /// Records grow up from here; their u16 offsets grow down from the end.
    pub const DATA: usize = 32;
    pub const KIND_HEADER: u8 = 1;
    pub const KIND_INDEX: u8 = 2;
    pub const KIND_LEAF: u8 = 3;
}

/// Header node (node 0) offsets.
pub mod hdr {
    #![allow(missing_docs)]
    /// Root node number, 0 = empty tree.
    pub const ROOT: usize = 32;
    pub const DEPTH: usize = 40;
    pub const NODES: usize = 48;
    pub const FREE_NODES: usize = 56;
    pub const RECORDS: usize = 64;
    pub const FIRST_LEAF: usize = 72;
    pub const LAST_LEAF: usize = 80;
    pub const NODE_SIZE: usize = 88;
    /// One bit per node: in use.
    pub const MAP: usize = 96;
}

/// Most nodes a tree can have (the header's map).
pub const MAX_NODES: u64 = ((NODE_SIZE - hdr::MAP) * 8) as u64;

/// Catalog record types and offsets.
pub mod rec {
    #![allow(missing_docs)]
    pub const FOLDER: u16 = 1;
    pub const FILE: u16 = 2;
    pub const FOLDER_THREAD: u16 = 3;
    pub const FILE_THREAD: u16 = 4;
    pub const TYPE: usize = 0;
    /// Bit 0: locked.
    pub const FLAGS: usize = 2;
    pub const CNID: usize = 4;
    /// Folders: items inside.
    pub const VALENCE: usize = 12;
    /// 5 x u64 ns: created, modified, attributes changed, accessed, backed up.
    pub const TIMES: usize = 20;
    /// 32 bytes of Finder information.
    pub const FINDER: usize = 60;
    pub const FOLDER_NAME: usize = 92;
    pub const DATA_FORK: usize = 92;
    pub const RSRC_FORK: usize = 92 + 144;
    pub const FILE_NAME: usize = 92 + 288;
    /// Threads: u64 parent, then u16 length and the name.
    pub const THREAD_PARENT: usize = 4;
    pub const THREAD_NAME: usize = 12;
}

/// Timestamps per item.
pub const TIME_COUNT: usize = 5;

/// Journal layout.
pub mod journal {
    #![allow(missing_docs)]
    pub const MAGIC: &[u8; 8] = b"eHFSjrnl";
    pub const TXN_MAGIC: &[u8; 8] = b"eHFStxn\0";
    pub const COMMIT_MAGIC: &[u8; 8] = b"eHFScmt\0";
    pub const HDR_SEQUENCE: usize = 8;
    pub const HDR_CRC: usize = 16;
    pub const TXN_SEQUENCE: usize = 8;
    pub const TXN_COUNT: usize = 16;
    pub const TXN_TARGETS: usize = 24;
    pub const COMMIT_SEQUENCE: usize = 8;
    pub const COMMIT_COUNT: usize = 16;
    pub const COMMIT_CRC: usize = 20;
    /// Most targets a descriptor block holds.
    pub const TXN_MAX: usize = (super::BLOCK_SIZE - TXN_TARGETS) / 8;
}

/// Metadata blocks one transaction may change.
pub const TXN_BLOCKS: usize = 256;

/// The Unicode version of the name tables (decision E2).
pub const UNICODE_VERSION: u32 = 0x0F_0000;

/// Little-endian reads and writes.
pub fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// See [`le16`].
pub fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// See [`le16`].
pub fn le64(b: &[u8], at: usize) -> u64 {
    u64::from(le32(b, at)) | u64::from(le32(b, at + 4)) << 32
}

/// See [`le16`].
pub fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// See [`le16`].
pub fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// See [`le16`].
pub fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Big-endian u64 (for keys).
pub fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}
