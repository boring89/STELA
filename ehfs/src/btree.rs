// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The B+-trees (catalog, extents, attributes; 68k_HAL `docs/ehfs.md`
//! sections 4 and 11.2).
//!
//! Node 0 of a tree's file is the header node (root, depth, counts, a map
//! of the nodes in use). Every other node in use is an index node
//! (separator key -> child node number, u64) or a leaf (key -> value); the
//! nodes of one level are linked in key order. A record is a u16 key
//! length, the key, a u16 value length and the value, packed from byte 32
//! up; a table of u16 record offsets grows down from the end of the node,
//! in key order. Keys compare as bytes, the shorter first when one is the
//! start of the other.
//!
//! A child's separator is its first key when it was split off; the
//! leftmost child of a new root gets the empty key; when the first record
//! of an index node is deleted, the new first record takes the old key,
//! so the first key of an index node is always the lower bound of its
//! subtree. Nodes are not merged: an empty node is freed, and a root with
//! one child is replaced by the child. A tree's file grows by doubling (at
//! most 8 runs, the superblock's fork descriptor).

use crate::Error;
use crate::crc32c::crc32c;
use crate::format::*;
use crate::volume::{Volume, inline_block};
use alloc::vec;
use alloc::vec::Vec;
use block::BlockDevice;
use core::cmp::Ordering;

/// Longest key.
pub const KEY_MAX: usize = 1100;
/// Longest value.
pub const VALUE_MAX: usize = 4096;
const MAX_DEPTH: usize = 16;
/// Key length, value length, offset entry.
const REC_OVERHEAD: usize = 6;
const HALF: usize = (NODE_SIZE - node::DATA) / 2;

/// A node buffer.
pub(crate) type Node = Vec<u8>;

fn new_node() -> Node {
    vec![0u8; NODE_SIZE]
}

/// Stores a node's CRC.
pub(crate) fn seal(n: &mut [u8]) {
    let crc = crc32c(0, &n[4..]);
    put32(n, node::CRC, crc);
}

/// Whether a node's CRC is right.
pub(crate) fn intact(n: &[u8]) -> bool {
    le32(n, node::CRC) == crc32c(0, &n[4..])
}

/// The header node of an empty tree of `nodes` nodes.
pub(crate) fn tree_header(nodes: u64) -> Node {
    let mut n = new_node();
    n[node::KIND] = node::KIND_HEADER;
    put64(&mut n, hdr::NODES, nodes);
    put64(&mut n, hdr::FREE_NODES, nodes - 1);
    put32(&mut n, hdr::NODE_SIZE, NODE_SIZE as u32);
    n[hdr::MAP] = 1;
    seal(&mut n);
    n
}

pub(crate) fn count(n: &[u8]) -> usize {
    usize::from(le16(n, node::COUNT))
}

fn offset_of(n: &[u8], i: usize) -> usize {
    usize::from(le16(n, NODE_SIZE - 2 * (i + 1)))
}

fn set_offset(n: &mut [u8], i: usize, off: usize) {
    put16(n, NODE_SIZE - 2 * (i + 1), off as u16);
}

pub(crate) fn key_of(n: &[u8], i: usize) -> &[u8] {
    let o = offset_of(n, i);
    let k = usize::from(le16(n, o));
    &n[o + 2..o + 2 + k]
}

pub(crate) fn value_of(n: &[u8], i: usize) -> &[u8] {
    let o = offset_of(n, i);
    let k = usize::from(le16(n, o));
    let v = usize::from(le16(n, o + 2 + k));
    &n[o + 4 + k..o + 4 + k + v]
}

fn size_of(n: &[u8], i: usize) -> usize {
    4 + key_of(n, i).len() + value_of(n, i).len()
}

fn free_space(n: &[u8]) -> usize {
    NODE_SIZE - 2 * count(n) - usize::from(le16(n, node::FREE))
}

pub(crate) fn child_of(n: &[u8], i: usize) -> u64 {
    le64(value_of(n, i), 0)
}

/// Byte order, the shorter first on a common prefix.
pub(crate) fn compare(a: &[u8], b: &[u8]) -> Ordering {
    a.cmp(b)
}

/// The first record whose key is >= `key` (count if none), and whether it
/// is equal.
fn lower_bound(n: &[u8], key: &[u8]) -> (usize, bool) {
    let (mut lo, mut hi) = (0, count(n));
    while lo < hi {
        let mid = (lo + hi) / 2;
        match compare(key_of(n, mid), key) {
            Ordering::Equal => return (mid, true),
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
        }
    }
    (lo, false)
}

/// The child of an index node to follow for `key`.
fn child_index(n: &[u8], key: &[u8]) -> usize {
    match lower_bound(n, key) {
        (i, true) => i,
        (0, false) => 0,
        (i, false) => i - 1,
    }
}

fn node_init(n: &mut [u8], kind: u8, height: u8) {
    n.fill(0);
    n[node::KIND] = kind;
    n[node::HEIGHT] = height;
    put16(n, node::FREE, node::DATA as u16);
}

/// Puts a record at position `i`; the caller made sure it fits.
fn node_insert(n: &mut [u8], i: usize, key: &[u8], value: &[u8]) {
    let c = count(n);
    let at = usize::from(le16(n, node::FREE));
    put16(n, at, key.len() as u16);
    n[at + 2..at + 2 + key.len()].copy_from_slice(key);
    put16(n, at + 2 + key.len(), value.len() as u16);
    n[at + 4 + key.len()..at + 4 + key.len() + value.len()].copy_from_slice(value);
    for j in (i + 1..=c).rev() {
        let o = offset_of(n, j - 1);
        set_offset(n, j, o);
    }
    set_offset(n, i, at);
    put16(n, node::COUNT, (c + 1) as u16);
    put16(n, node::FREE, (at + 4 + key.len() + value.len()) as u16);
}

/// Takes record `i` out and closes the gap.
fn node_remove(n: &mut [u8], i: usize) {
    let c = count(n);
    let at = offset_of(n, i);
    let size = size_of(n, i);
    let end = usize::from(le16(n, node::FREE));
    n.copy_within(at + size..end, at);
    for j in i..c - 1 {
        let o = offset_of(n, j + 1);
        set_offset(n, j, o);
    }
    for j in 0..c - 1 {
        let o = offset_of(n, j);
        if o > at {
            set_offset(n, j, o - size);
        }
    }
    set_offset(n, c - 1, 0);
    put16(n, node::COUNT, (c - 1) as u16);
    put16(n, node::FREE, (end - size) as u16);
}

struct Path {
    node: [u64; MAX_DEPTH],
    index: [usize; MAX_DEPTH],
    depth: usize,
}

impl<D: BlockDevice> Volume<D> {
    fn node_io(&mut self, tree: Special, n: u64, buf: &mut [u8], write: bool) -> Result<(), Error> {
        let fork_desc: Vec<u8> = self.sb_fork(tree).to_vec();
        if n >= le64(&fork_desc, fork::BLOCKS) / NODE_BLOCKS {
            return Err(Error::Corrupt);
        }
        if write {
            seal(buf);
        }
        for k in 0..NODE_BLOCKS {
            let b = inline_block(&fork_desc, n * NODE_BLOCKS + k)?;
            let part = &mut buf[k as usize * BLOCK_SIZE..(k as usize + 1) * BLOCK_SIZE];
            if write {
                self.write_meta(b, part)?;
            } else {
                self.read_block(b, part)?;
            }
        }
        if !write && !intact(buf) {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    pub(crate) fn read_node(&mut self, tree: Special, n: u64) -> Result<Node, Error> {
        let mut buf = new_node();
        self.node_io(tree, n, &mut buf, false)?;
        if n != 0 && buf[node::KIND] != node::KIND_INDEX && buf[node::KIND] != node::KIND_LEAF {
            return Err(Error::Corrupt);
        }
        Ok(buf)
    }

    fn write_node(&mut self, tree: Special, n: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.node_io(tree, n, buf, true)
    }

    pub(crate) fn read_header(&mut self, tree: Special) -> Result<Node, Error> {
        let mut buf = new_node();
        self.node_io(tree, 0, &mut buf, false)?;
        if buf[node::KIND] != node::KIND_HEADER {
            return Err(Error::Corrupt);
        }
        Ok(buf)
    }

    /// Doubles the tree's file.
    fn grow_tree(&mut self, tree: Special, h: &mut [u8]) -> Result<(), Error> {
        let nodes = le64(h, hdr::NODES);
        let more = nodes.min(MAX_NODES - nodes);
        if more == 0 {
            return Err(Error::NoSpace);
        }
        let mut need = more * NODE_BLOCKS;
        while need > 0 {
            let f = self.sb_fork(tree).to_vec();
            let mut used = 0;
            let mut last_end = 0;
            while used < INLINE_EXTENTS && le64(&f, fork::EXTENT + 16 * used + 8) != 0 {
                last_end =
                    le64(&f, fork::EXTENT + 16 * used) + le64(&f, fork::EXTENT + 16 * used + 8);
                used += 1;
            }
            let (start, cnt) = self.alloc(last_end, need)?;
            let fd = self.sb_fork_mut(tree);
            if used > 0 && start == last_end {
                let e = fork::EXTENT + 16 * (used - 1);
                let c = le64(fd, e + 8);
                put64(fd, e + 8, c + cnt);
            } else if used < INLINE_EXTENTS {
                put64(fd, fork::EXTENT + 16 * used, start);
                put64(fd, fork::EXTENT + 16 * used + 8, cnt);
            } else {
                self.free(start, cnt)?;
                return Err(Error::NoSpace);
            }
            let fd = self.sb_fork_mut(tree);
            let blocks = le64(fd, fork::BLOCKS) + cnt;
            put64(fd, fork::BLOCKS, blocks);
            put64(fd, fork::LENGTH, blocks * BLOCK_SIZE as u64);
            need -= cnt;
        }
        put64(h, hdr::NODES, nodes + more);
        let free = le64(h, hdr::FREE_NODES);
        put64(h, hdr::FREE_NODES, free + more);
        Ok(())
    }

    fn alloc_node(&mut self, tree: Special, h: &mut [u8]) -> Result<u64, Error> {
        if le64(h, hdr::FREE_NODES) == 0 {
            self.grow_tree(tree, h)?;
        }
        let nodes = le64(h, hdr::NODES);
        for i in 1..nodes {
            let byte = &mut h[hdr::MAP + (i / 8) as usize];
            if *byte & (1 << (i % 8)) == 0 {
                *byte |= 1 << (i % 8);
                let free = le64(h, hdr::FREE_NODES);
                put64(h, hdr::FREE_NODES, free - 1);
                return Ok(i);
            }
        }
        Err(Error::Corrupt)
    }

    fn free_node(h: &mut [u8], n: u64) {
        h[hdr::MAP + (n / 8) as usize] &= !(1 << (n % 8));
        let free = le64(h, hdr::FREE_NODES);
        put64(h, hdr::FREE_NODES, free + 1);
    }

    /// From the root to the leaf for `key`; returns the leaf.
    fn descend(
        &mut self,
        tree: Special,
        h: &[u8],
        key: &[u8],
        path: &mut Path,
    ) -> Result<Node, Error> {
        let mut n = le64(h, hdr::ROOT);
        let depth = le32(h, hdr::DEPTH) as usize;
        if depth == 0 || depth > MAX_DEPTH {
            return Err(Error::Corrupt);
        }
        path.depth = depth;
        let mut buf = new_node();
        for level in 0..depth {
            buf = self.read_node(tree, n)?;
            if usize::from(buf[node::HEIGHT]) != depth - level
                || count(&buf) == 0
                || (level + 1 < depth) != (buf[node::KIND] == node::KIND_INDEX)
            {
                return Err(Error::Corrupt);
            }
            path.node[level] = n;
            if level + 1 < depth {
                path.index[level] = child_index(&buf, key);
                n = child_of(&buf, path.index[level]);
            }
        }
        Ok(buf)
    }

    fn new_path() -> Path {
        Path {
            node: [0; MAX_DEPTH],
            index: [0; MAX_DEPTH],
            depth: 0,
        }
    }

    /// The value of `key`.
    pub(crate) fn bt_find(&mut self, tree: Special, key: &[u8]) -> Result<Vec<u8>, Error> {
        let h = self.read_header(tree)?;
        if le64(&h, hdr::ROOT) == 0 {
            return Err(Error::NotFound);
        }
        let mut path = Self::new_path();
        let leaf = self.descend(tree, &h, key, &mut path)?;
        match lower_bound(&leaf, key) {
            (i, true) => Ok(value_of(&leaf, i).to_vec()),
            _ => Err(Error::NotFound),
        }
    }

    /// The record with the greatest key <= `key`: (key, value).
    pub(crate) fn bt_find_le(
        &mut self,
        tree: Special,
        key: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let h = self.read_header(tree)?;
        if le64(&h, hdr::ROOT) == 0 {
            return Err(Error::NotFound);
        }
        let mut path = Self::new_path();
        let mut leaf = self.descend(tree, &h, key, &mut path)?;
        let (mut i, exact) = lower_bound(&leaf, key);
        if !exact {
            if i == 0 {
                let prev = le64(&leaf, node::PREV);
                if prev == 0 {
                    return Err(Error::NotFound);
                }
                leaf = self.read_node(tree, prev)?;
                if count(&leaf) == 0 {
                    return Err(Error::Corrupt);
                }
                i = count(&leaf);
            }
            i -= 1;
        }
        Ok((key_of(&leaf, i).to_vec(), value_of(&leaf, i).to_vec()))
    }

    /// The first record whose key is > `key` (>= with `inclusive`).
    pub(crate) fn bt_next(
        &mut self,
        tree: Special,
        key: &[u8],
        inclusive: bool,
    ) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let h = self.read_header(tree)?;
        if le64(&h, hdr::ROOT) == 0 {
            return Err(Error::NotFound);
        }
        let mut path = Self::new_path();
        let mut leaf = self.descend(tree, &h, key, &mut path)?;
        let (mut i, exact) = lower_bound(&leaf, key);
        if exact && !inclusive {
            i += 1;
        }
        while i >= count(&leaf) {
            let next = le64(&leaf, node::NEXT);
            if next == 0 {
                return Err(Error::NotFound);
            }
            leaf = self.read_node(tree, next)?;
            i = 0;
        }
        Ok((key_of(&leaf, i).to_vec(), value_of(&leaf, i).to_vec()))
    }

    /// Records in the tree.
    pub(crate) fn bt_records(&mut self, tree: Special) -> u64 {
        self.read_header(tree).map_or(0, |h| le64(&h, hdr::RECORDS))
    }

    /// Links node `n` (`right`) after `left_n` (`left`) on their level.
    #[allow(clippy::too_many_arguments)]
    fn link_after(
        &mut self,
        tree: Special,
        h: &mut [u8],
        left_n: u64,
        left: &mut [u8],
        n: u64,
        right: &mut [u8],
        leaf: bool,
    ) -> Result<(), Error> {
        let next = le64(left, node::NEXT);
        put64(right, node::PREV, left_n);
        put64(right, node::NEXT, next);
        put64(left, node::NEXT, n);
        if next != 0 {
            let mut after = self.read_node(tree, next)?;
            put64(&mut after, node::PREV, n);
            self.write_node(tree, next, &mut after)?;
        } else if leaf {
            put64(h, hdr::LAST_LEAF, n);
        }
        Ok(())
    }

    /// Puts (key, value) into `path.node[level]` (`node`), splitting and
    /// going up as needed.
    #[allow(clippy::too_many_arguments)]
    fn insert_at(
        &mut self,
        tree: Special,
        h: &mut [u8],
        path: &Path,
        mut level: usize,
        mut node_buf: Node,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), Error> {
        let mut key = key.to_vec();
        let mut value = value.to_vec();
        loop {
            let n = path.node[level];
            let (i, _) = lower_bound(&node_buf, &key);
            let leaf = node_buf[node::KIND] == node::KIND_LEAF;
            if free_space(&node_buf) >= key.len() + value.len() + REC_OVERHEAD {
                node_insert(&mut node_buf, i, &key, &value);
                return self.write_node(tree, n, &mut node_buf);
            }
            // Split: the records and the new one, in order, shared out
            // between this node and a new one after it, about half the
            // bytes each (both get at least one record).
            let right = self.alloc_node(tree, h)?;
            let copy = node_buf.clone();
            let c = count(&copy);
            let new_size = key.len() + value.len() + 6;
            let rec_size = |j: usize| -> usize {
                if j == i {
                    new_size
                } else {
                    size_of(&copy, if j < i { j } else { j - 1 }) + 2
                }
            };
            let mut total = new_size;
            for j in 0..c {
                total += size_of(&copy, j) + 2;
            }
            let (mut at, mut sum) = (0, 0);
            while at < c && sum + rec_size(at) <= total / 2 {
                sum += rec_size(at);
                at += 1;
            }
            if at == 0 {
                at = 1;
            }
            let prev = le64(&copy, node::PREV);
            let next = le64(&copy, node::NEXT);
            let mut rn = new_node();
            node_init(&mut node_buf, copy[node::KIND], copy[node::HEIGHT]);
            node_init(&mut rn, copy[node::KIND], copy[node::HEIGHT]);
            put64(&mut node_buf, node::PREV, prev);
            put64(&mut node_buf, node::NEXT, next);
            for j in 0..=c {
                let (to, pos) = if j < at {
                    (&mut node_buf, j)
                } else {
                    (&mut rn, j - at)
                };
                if j == i {
                    node_insert(to, pos, &key, &value);
                } else {
                    let src = if j < i { j } else { j - 1 };
                    node_insert(to, pos, key_of(&copy, src), value_of(&copy, src));
                }
            }
            let sep = key_of(&rn, 0).to_vec();
            self.link_after(tree, h, n, &mut node_buf, right, &mut rn, leaf)?;
            self.write_node(tree, n, &mut node_buf)?;
            self.write_node(tree, right, &mut rn)?;
            let child = right.to_le_bytes();
            if level == 0 {
                // A new root above the two.
                let root = self.alloc_node(tree, h)?;
                let mut r = new_node();
                node_init(&mut r, node::KIND_INDEX, (path.depth + 1) as u8);
                node_insert(&mut r, 0, &[], &n.to_le_bytes());
                node_insert(&mut r, 1, &sep, &child);
                put64(h, hdr::ROOT, root);
                put32(h, hdr::DEPTH, (path.depth + 1) as u32);
                return self.write_node(tree, root, &mut r);
            }
            level -= 1;
            node_buf = self.read_node(tree, path.node[level])?;
            key = sep;
            value = child.to_vec();
        }
    }

    /// Adds a record (`Exists` if the key is there).
    pub(crate) fn bt_insert(
        &mut self,
        tree: Special,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), Error> {
        if key.len() > KEY_MAX
            || value.len() > VALUE_MAX
            || key.len() + value.len() + REC_OVERHEAD > HALF
        {
            return Err(Error::Invalid);
        }
        let mut h = self.read_header(tree)?;
        if le64(&h, hdr::ROOT) == 0 {
            let n = self.alloc_node(tree, &mut h)?;
            let mut leaf = new_node();
            node_init(&mut leaf, node::KIND_LEAF, 1);
            node_insert(&mut leaf, 0, key, value);
            self.write_node(tree, n, &mut leaf)?;
            put64(&mut h, hdr::ROOT, n);
            put32(&mut h, hdr::DEPTH, 1);
            put64(&mut h, hdr::FIRST_LEAF, n);
            put64(&mut h, hdr::LAST_LEAF, n);
        } else {
            let mut path = Self::new_path();
            let leaf = self.descend(tree, &h, key, &mut path)?;
            if lower_bound(&leaf, key).1 {
                return Err(Error::Exists);
            }
            self.insert_at(tree, &mut h, &path, path.depth - 1, leaf, key, value)?;
        }
        let records = le64(&h, hdr::RECORDS);
        put64(&mut h, hdr::RECORDS, records + 1);
        self.write_node(tree, 0, &mut h)
    }

    /// Takes node `n` (`buf`) out of its level's chain.
    fn unlink_node(
        &mut self,
        tree: Special,
        h: &mut [u8],
        buf: &[u8],
        leaf: bool,
    ) -> Result<(), Error> {
        let prev = le64(buf, node::PREV);
        let next = le64(buf, node::NEXT);
        if prev != 0 {
            let mut p = self.read_node(tree, prev)?;
            put64(&mut p, node::NEXT, next);
            self.write_node(tree, prev, &mut p)?;
        } else if leaf {
            put64(h, hdr::FIRST_LEAF, next);
        }
        if next != 0 {
            let mut x = self.read_node(tree, next)?;
            put64(&mut x, node::PREV, prev);
            self.write_node(tree, next, &mut x)?;
        } else if leaf {
            put64(h, hdr::LAST_LEAF, prev);
        }
        Ok(())
    }

    /// Removes a record (`NotFound` if the key is not there).
    pub(crate) fn bt_delete(&mut self, tree: Special, key: &[u8]) -> Result<(), Error> {
        let mut h = self.read_header(tree)?;
        if le64(&h, hdr::ROOT) == 0 {
            return Err(Error::NotFound);
        }
        let mut path = Self::new_path();
        let mut buf = self.descend(tree, &h, key, &mut path)?;
        let (mut i, exact) = lower_bound(&buf, key);
        if !exact {
            return Err(Error::NotFound);
        }
        let records = le64(&h, hdr::RECORDS);
        put64(&mut h, hdr::RECORDS, records - 1);
        let mut level = path.depth - 1;
        loop {
            let n = path.node[level];
            let bound = key_of(&buf, 0).to_vec();
            node_remove(&mut buf, i);
            if count(&buf) > 0 {
                // An index node keeps its first key: the lower bound of
                // everything below it.
                if i == 0 && buf[node::KIND] == node::KIND_INDEX {
                    let child = value_of(&buf, 0).to_vec();
                    node_remove(&mut buf, 0);
                    node_insert(&mut buf, 0, &bound, &child);
                }
                self.write_node(tree, n, &mut buf)?;
                break;
            }
            self.unlink_node(tree, &mut h, &buf, buf[node::KIND] == node::KIND_LEAF)?;
            Self::free_node(&mut h, n);
            if level == 0 {
                put64(&mut h, hdr::ROOT, 0);
                put32(&mut h, hdr::DEPTH, 0);
                put64(&mut h, hdr::FIRST_LEAF, 0);
                put64(&mut h, hdr::LAST_LEAF, 0);
                break;
            }
            buf = self.read_node(tree, path.node[level - 1])?;
            i = path.index[level - 1];
            level -= 1;
        }
        // A root index node with one child: the child becomes the root.
        while le32(&h, hdr::DEPTH) > 1 {
            let root = le64(&h, hdr::ROOT);
            let r = self.read_node(tree, root)?;
            if count(&r) != 1 {
                break;
            }
            put64(&mut h, hdr::ROOT, child_of(&r, 0));
            let depth = le32(&h, hdr::DEPTH);
            put32(&mut h, hdr::DEPTH, depth - 1);
            Self::free_node(&mut h, root);
        }
        self.write_node(tree, 0, &mut h)
    }

    /// Changes the value of a record (`NotFound` if the key is not there).
    pub(crate) fn bt_replace(
        &mut self,
        tree: Special,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), Error> {
        let h = self.read_header(tree)?;
        if le64(&h, hdr::ROOT) == 0 {
            return Err(Error::NotFound);
        }
        let mut path = Self::new_path();
        let mut leaf = self.descend(tree, &h, key, &mut path)?;
        let (i, exact) = lower_bound(&leaf, key);
        if !exact {
            return Err(Error::NotFound);
        }
        if value_of(&leaf, i).len() == value.len() {
            let o = offset_of(&leaf, i);
            let k = key_of(&leaf, i).len();
            leaf[o + 4 + k..o + 4 + k + value.len()].copy_from_slice(value);
            return self.write_node(tree, path.node[path.depth - 1], &mut leaf);
        }
        self.bt_delete(tree, key)?;
        self.bt_insert(tree, key, value)
    }

    /// Every record with key >= `start`, in order, until `visit` returns
    /// `true` (stop).
    pub(crate) fn bt_walk(
        &mut self,
        tree: Special,
        start: &[u8],
        mut visit: impl FnMut(&[u8], &[u8]) -> bool,
    ) -> Result<(), Error> {
        let h = self.read_header(tree)?;
        if le64(&h, hdr::ROOT) == 0 {
            return Ok(());
        }
        let mut path = Self::new_path();
        let mut leaf = self.descend(tree, &h, start, &mut path)?;
        let (mut i, _) = lower_bound(&leaf, start);
        loop {
            while i < count(&leaf) {
                if visit(key_of(&leaf, i), value_of(&leaf, i)) {
                    return Ok(());
                }
                i += 1;
            }
            let next = le64(&leaf, node::NEXT);
            if next == 0 {
                return Ok(());
            }
            leaf = self.read_node(tree, next)?;
            i = 0;
        }
    }
}
