// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! fsck: checks a volume's structures (it reads, it does not repair),
//! the same checks as 68k_HAL's `check.c`: the superblock copy, the
//! special files, every tree (order, bounds, links, counts, node map), the
//! catalog (records and threads agree, valences, counts, CNIDs), the forks
//! and extents records, the attributes, and the bitmap against everything
//! that owns blocks.

use crate::btree::{child_of, count, intact, key_of, value_of};
use crate::catalog::{decode, decode_thread, make_key};
use crate::crc32c::crc32c;
use crate::format::*;
use crate::volume::{Volume, inline_block};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use block::BlockDevice;

/// What fsck found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Problems found.
    pub errors: u32,
    /// The first one.
    pub first: String,
}

impl Report {
    fn problem(&mut self, what: &str) {
        if self.errors == 0 {
            self.first = String::from(what);
        }
        self.errors += 1;
    }

    /// Whether nothing was found.
    pub fn clean(&self) -> bool {
        self.errors == 0
    }
}

const MAX_DEPTH: u32 = 16;

impl<D: BlockDevice> Volume<D> {
    /// Checks the volume.
    pub fn check(&mut self) -> Report {
        let mut report = Report::default();
        let mut b = vec![0u8; BLOCK_SIZE];
        if self.read_block(self.blocks - 1, &mut b).is_err() {
            report.problem("the superblock's copy cannot be read");
        } else {
            let copy = &b[SB_OFFSET..];
            if &copy[..8] != MAGIC
                || le32(copy, sb::CRC) != crc32c(0, &copy[..sb::CRC])
                || copy[sb::UUID..sb::UUID + 16] != self.sb[sb::UUID..sb::UUID + 16]
                || le64(copy, sb::BLOCK_COUNT) != self.blocks
            {
                report.problem("the superblock's copy does not match");
            }
        }
        if self.sb_get64(sb::FREE_BLOCKS) > self.blocks {
            report.problem("more free blocks than blocks");
        }
        self.check_special_forks(&mut report);
        for t in [Special::Catalog, Special::Extents, Special::Attributes] {
            self.check_tree_header(t, &mut report);
        }
        if report.clean() {
            for t in [Special::Catalog, Special::Extents, Special::Attributes] {
                self.check_tree(t, &mut report);
            }
        }
        let mut owners: Vec<(u64, u64)> = Vec::new();
        if report.clean() {
            self.check_catalog(&mut report);
        }
        if report.clean() {
            owners = self.check_files(&mut report);
        }
        if report.clean() {
            self.check_bitmap(owners, &mut report);
        }
        report
    }

    fn check_special_forks(&self, report: &mut Report) {
        for f in [
            Special::Bitmap,
            Special::Catalog,
            Special::Extents,
            Special::Attributes,
            Special::Journal,
            Special::BadBlocks,
        ] {
            let d = self.sb_fork(f);
            let sum: u64 = (0..INLINE_EXTENTS)
                .map(|i| le64(d, fork::EXTENT + 16 * i + 8))
                .sum();
            if sum != le64(d, fork::BLOCKS) {
                report.problem("a special file's extents do not add up to its block count");
            }
            if f != Special::BadBlocks && le64(d, fork::LENGTH) != sum * BLOCK_SIZE as u64 {
                report.problem("a special file's length is not its blocks");
            }
        }
        let j = self.sb_fork(Special::Journal);
        if le64(j, fork::EXTENT + 8) != le64(j, fork::BLOCKS) {
            report.problem("the journal is not one run of blocks");
        }
    }

    fn check_tree_header(&mut self, tree: Special, report: &mut Report) {
        let d = self.sb_fork(tree).to_vec();
        let nodes = le64(&d, fork::BLOCKS) / NODE_BLOCKS;
        let mut n = vec![0u8; NODE_SIZE];
        for k in 0..NODE_BLOCKS {
            let part = &mut n[k as usize * BLOCK_SIZE..(k as usize + 1) * BLOCK_SIZE];
            if inline_block(&d, k)
                .and_then(|b| self.read_block(b, part))
                .is_err()
            {
                report.problem("a B-tree's header node cannot be read");
                return;
            }
        }
        if !intact(&n) || n[node::KIND] != node::KIND_HEADER {
            report.problem("a B-tree's header node is damaged");
        } else if le64(&n, hdr::NODES) != nodes
            || le32(&n, hdr::NODE_SIZE) != NODE_SIZE as u32
            || n[hdr::MAP] & 1 == 0
        {
            report.problem("a B-tree's header node does not match its file");
        }
    }

    /// One tree, level by level from the root.
    fn check_tree(&mut self, tree: Special, report: &mut Report) {
        let Ok(h) = self.read_header(tree) else {
            report.problem("a B-tree's header node cannot be read");
            return;
        };
        let nodes = le64(&h, hdr::NODES);
        let root = le64(&h, hdr::ROOT);
        let depth = le32(&h, hdr::DEPTH);
        let mut seen = vec![0u8; nodes.div_ceil(8) as usize + 1];
        seen[0] = 1;
        let (mut records, mut used) = (0u64, 1u64);
        if (root == 0) != (depth == 0) || depth > MAX_DEPTH || root >= nodes {
            report.problem("a B-tree's root and depth do not match");
            return;
        }
        let mut last_key: Option<Vec<u8>>;
        let mut first = root;
        let mut level = 0;
        while level < depth && first != 0 {
            let height = depth - level;
            let leaf = height == 1;
            let (mut n, mut prev, mut expected_child, mut next_first) = (first, 0u64, 0u64, 0u64);
            last_key = None;
            if leaf && le64(&h, hdr::FIRST_LEAF) != first {
                report.problem("a B-tree's first leaf is wrong");
            }
            while n != 0 {
                if n >= nodes || seen[(n / 8) as usize] & (1 << (n % 8)) != 0 {
                    report.problem("a B-tree's nodes loop or point outside it");
                    return;
                }
                seen[(n / 8) as usize] |= 1 << (n % 8);
                used += 1;
                let Ok(buf) = self.read_node(tree, n) else {
                    report.problem("a B-tree node is damaged");
                    return;
                };
                if u32::from(buf[node::HEIGHT]) != height
                    || (buf[node::KIND] == node::KIND_LEAF) != leaf
                    || count(&buf) == 0
                {
                    report.problem("a B-tree node has the wrong kind, height or no records");
                }
                if le64(&buf, node::PREV) != prev {
                    report.problem("a B-tree node's previous link is wrong");
                }
                let mut used_bytes = node::DATA;
                for i in 0..count(&buf) {
                    let k = key_of(&buf, i);
                    used_bytes += 4 + k.len() + value_of(&buf, i).len();
                    if k.len() > crate::btree::KEY_MAX
                        || last_key.as_deref().is_some_and(|l| l >= k)
                    {
                        report.problem("a B-tree's keys are not in order");
                    }
                    last_key = Some(k.to_vec());
                    if !leaf {
                        if value_of(&buf, i).len() != 8 {
                            report.problem("a B-tree index record is not a node number");
                            return;
                        }
                        let c = child_of(&buf, i);
                        if expected_child == 0 && next_first == 0 {
                            next_first = c;
                        } else if c != expected_child {
                            report.problem("a B-tree's children are not the next level's chain");
                        }
                        let Ok(child) = self.read_node(tree, c) else {
                            report.problem("a B-tree node is damaged");
                            return;
                        };
                        if count(&child) > 0 && key_of(&child, 0) < k {
                            report.problem("a B-tree child holds keys below its separator");
                        }
                        expected_child = le64(&child, node::NEXT);
                    }
                }
                if used_bytes != usize::from(le16(&buf, node::FREE)) {
                    report.problem("a B-tree node's free space is wrong");
                }
                if leaf {
                    records += count(&buf) as u64;
                }
                prev = n;
                n = le64(&buf, node::NEXT);
            }
            if !leaf && expected_child != 0 {
                report.problem("a B-tree level has nodes no parent points to");
            }
            if leaf && le64(&h, hdr::LAST_LEAF) != prev {
                report.problem("a B-tree's last leaf is wrong");
            }
            first = if leaf { 0 } else { next_first };
            level += 1;
        }
        if records != le64(&h, hdr::RECORDS) {
            report.problem("a B-tree's record count is wrong");
        }
        for i in 0..nodes {
            let mapped = h[hdr::MAP + (i / 8) as usize] >> (i % 8) & 1 != 0;
            let reached = seen[(i / 8) as usize] >> (i % 8) & 1 != 0;
            if mapped != reached {
                report.problem(if mapped {
                    "a B-tree node is marked in use but not in the tree"
                } else {
                    "a B-tree node in the tree is marked free"
                });
                break;
            }
        }
        if nodes - used != le64(&h, hdr::FREE_NODES) {
            report.problem("a B-tree's free node count is wrong");
        }
    }

    fn check_catalog(&mut self, report: &mut Report) {
        let Ok(records) = self.cat_records() else {
            report.problem("the catalog cannot be read");
            return;
        };
        let next_cnid = self.sb_get64(sb::NEXT_CNID);
        let (mut files, mut folders, mut threads, mut items) = (0u64, 0u64, 0u64, 0u64);
        for (key, value) in &records {
            if key.len() == 8 {
                threads += 1;
                let cnid = be64(key);
                let Ok((parent, name, is_dir)) = decode_thread(value) else {
                    report.problem("a catalog thread is damaged");
                    continue;
                };
                if cnid < cnid::ROOT || cnid >= next_cnid {
                    report.problem("a catalog thread's CNID is out of range");
                }
                match self.lookup(parent, &name) {
                    Ok(e) if e.cnid == cnid && e.is_dir == is_dir => {}
                    _ => report.problem("a catalog thread does not lead to its record"),
                }
                continue;
            }
            items += 1;
            if key.len() < 8 {
                report.problem("a catalog record is damaged");
                continue;
            }
            let parent = be64(key);
            let Ok(e) = decode(value, parent) else {
                report.problem("a catalog record is damaged");
                continue;
            };
            if make_key(parent, &e.name).ok().as_ref() != Some(key) {
                report.problem("a catalog record's key does not match its name");
            }
            let bad_cnid = if e.cnid == cnid::ROOT {
                parent != cnid::ROOT_PARENT || !e.is_dir
            } else {
                e.cnid < cnid::FIRST || e.cnid >= next_cnid
            };
            if bad_cnid {
                report.problem("a catalog record's CNID is out of range");
            }
            if e.cnid > cnid::MAC_MAX {
                report.problem("a CNID does not fit the Mac's 32 bits");
            }
            match self.get(e.cnid) {
                Ok(back) if back.parent == parent => {}
                _ => report.problem("a catalog record has no thread back to it"),
            }
            if e.cnid != cnid::ROOT && !self.get(parent).is_ok_and(|up| up.is_dir) {
                report.problem("a catalog record's parent is not a folder");
            }
            if e.is_dir {
                if e.cnid != cnid::ROOT {
                    folders += 1;
                }
                let inside = records
                    .iter()
                    .filter(|(k, _)| k.len() > 8 && be64(k) == e.cnid)
                    .count() as u64;
                if inside != e.valence {
                    report.problem("a folder's item count is wrong");
                }
            } else {
                files += 1;
            }
        }
        if threads != items {
            report.problem("the catalog has more threads than records, or fewer");
        }
        if files != self.sb_get64(sb::FILE_COUNT) || folders != self.sb_get64(sb::FOLDER_COUNT) {
            report.problem("the volume's file or folder count is wrong");
        }
    }

    /// Forks and attributes; returns every run the files own.
    fn check_files(&mut self, report: &mut Report) -> Vec<(u64, u64)> {
        let mut runs = Vec::new();
        let Ok(records) = self.cat_records() else {
            report.problem("the files cannot be read");
            return runs;
        };
        let mut extent_records = 0;
        for (key, value) in &records {
            if key.len() <= 8 || le16(value, rec::TYPE) != rec::FILE {
                continue;
            }
            let Ok(e) = decode(value, be64(key)) else {
                continue;
            };
            for resource in [false, true] {
                let desc = *e.fork(resource);
                match self.fork_runs(&desc, e.cnid, resource) {
                    Ok((r, n)) => {
                        runs.extend(r);
                        extent_records += n;
                    }
                    Err(_) => {
                        report.problem("a file's runs do not add up to its blocks");
                        continue;
                    }
                }
                if le64(&desc, fork::LENGTH).div_ceil(BLOCK_SIZE as u64)
                    != le64(&desc, fork::BLOCKS)
                {
                    report.problem("a file's length does not match its blocks");
                }
            }
        }
        if extent_records != self.bt_records(Special::Extents) {
            report.problem("the extents tree has records of no file's fork");
        }
        let mut attrs = Vec::new();
        if self
            .bt_walk(Special::Attributes, &[], |k, v| {
                attrs.push((k.to_vec(), v.to_vec()));
                false
            })
            .is_err()
        {
            report.problem("the attributes cannot be read");
        }
        for (k, v) in attrs {
            if k.len() <= 8 || self.get(be64(&k)).is_err() {
                report.problem("an attribute belongs to no file or folder");
            }
            if v.len() < 6 || le16(&v, 0) != 1 || le32(&v, 2) as usize != v.len() - 6 {
                report.problem("an attribute is damaged");
            }
        }
        runs
    }

    /// The bitmap against every owner.
    fn check_bitmap(&mut self, files: Vec<(u64, u64)>, report: &mut Report) {
        let blocks = self.blocks;
        let mut expected = vec![0u8; blocks.div_ceil(8) as usize];
        let mut owners = vec![(0u64, 1u64), (blocks - 1, 1)];
        for f in [
            Special::Bitmap,
            Special::Catalog,
            Special::Extents,
            Special::Attributes,
            Special::Journal,
            Special::BadBlocks,
        ] {
            let d = self.sb_fork(f);
            for i in 0..INLINE_EXTENTS {
                let c = le64(d, fork::EXTENT + 16 * i + 8);
                if c != 0 {
                    owners.push((le64(d, fork::EXTENT + 16 * i), c));
                }
            }
        }
        owners.extend(files);
        for (start, count) in owners {
            let end = start.checked_add(count).filter(|&e| e <= blocks);
            let Some(end) = end else {
                report.problem("an extent goes past the end of the volume");
                continue;
            };
            for n in start..end {
                let byte = &mut expected[(n / 8) as usize];
                if *byte & (1 << (n % 8)) != 0 {
                    report.problem("two owners share a block");
                }
                *byte |= 1 << (n % 8);
            }
        }
        let mut used = 0;
        let bits = (BLOCK_SIZE * 8) as u64;
        let mut b = vec![0u8; BLOCK_SIZE];
        let d = self.sb_fork(Special::Bitmap).to_vec();
        for i in 0..blocks.div_ceil(bits) {
            if inline_block(&d, i)
                .and_then(|x| self.read_block(x, &mut b))
                .is_err()
            {
                report.problem("the bitmap cannot be read");
                return;
            }
            for n in i * bits..((i + 1) * bits).min(blocks) {
                let on_disk = b[((n - i * bits) / 8) as usize] >> (n % 8) & 1 != 0;
                let owned = expected[(n / 8) as usize] >> (n % 8) & 1 != 0;
                if on_disk != owned {
                    report.problem(if on_disk {
                        "a block is marked in use but nothing owns it"
                    } else {
                        "a block in use is marked free"
                    });
                }
                used += u64::from(on_disk);
            }
        }
        if report.clean() && used + self.sb_get64(sb::FREE_BLOCKS) != blocks {
            report.problem("the free block count does not match the bitmap");
        }
    }
}
