// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! File operations, each complete with its own transactions (68k_HAL
//! `docs/ehfs.md` section 11.2). Writing is ordered: the data goes to its
//! blocks first, then the transaction that gives the blocks to the file
//! (and changes its length) commits. Blocks a fork gets are filled with
//! zeros where nothing is written. Large writes, truncations and removals
//! are split into transactions of at most 256 file blocks; a crash between
//! them leaves a consistent, partly done result.

use crate::Error;
use crate::catalog::{Entry, be};
use crate::format::*;
use crate::volume::Volume;
use alloc::vec;
use alloc::vec::Vec;
use block::BlockDevice;

const CHUNK: u64 = 256;
/// u16 type (1 = inline), u32 length.
const ATTR_HEADER: usize = 6;
/// Longest attribute name.
pub const ATTR_NAME_MAX: usize = 255;
/// Largest attribute value (kept in its record).
pub const ATTR_VALUE_MAX: usize = 3072;

fn blocks_for(length: u64) -> u64 {
    length.div_ceil(BLOCK_SIZE as u64)
}

fn attr_key(cnid: u64, name: &[u8]) -> Vec<u8> {
    let mut k = be(cnid).to_vec();
    k.extend_from_slice(name);
    k
}

fn attr_name_ok(name: &[u8]) -> bool {
    !name.is_empty() && name.len() <= ATTR_NAME_MAX && !name.contains(&0)
}

impl<D: BlockDevice> Volume<D> {
    /// Makes a new empty file or folder.
    pub fn create(
        &mut self,
        parent: u64,
        name: &[u8],
        is_dir: bool,
        now: u64,
    ) -> Result<Entry, Error> {
        self.begin()?;
        let r = self.cat_create(parent, name, is_dir, now);
        self.finish(r)
    }

    /// Moves and/or renames an item.
    pub fn rename(
        &mut self,
        parent: u64,
        name: &[u8],
        new_parent: u64,
        new_name: &[u8],
        now: u64,
    ) -> Result<(), Error> {
        self.begin()?;
        let r = self.cat_move(parent, name, new_parent, new_name, now);
        self.finish(r)
    }

    /// Writes back the flags, times and Finder information of an item.
    pub fn set_info(&mut self, entry: &Entry) -> Result<(), Error> {
        self.begin()?;
        let r = (|| {
            let mut e = self.get(entry.cnid)?;
            e.flags = entry.flags;
            e.times = entry.times;
            e.finder = entry.finder;
            self.cat_update(&e)
        })();
        self.finish(r)
    }

    // ---- Attributes ----

    /// Sets an extended attribute (name: 1-255 bytes without NUL, case
    /// kept; value up to 3 KiB).
    pub fn attr_set(&mut self, cnid: u64, name: &[u8], value: &[u8]) -> Result<(), Error> {
        if !attr_name_ok(name) || value.len() > ATTR_VALUE_MAX {
            return Err(Error::Invalid);
        }
        self.begin()?;
        let r = (|| {
            self.get(cnid)?;
            let key = attr_key(cnid, name);
            let mut record = vec![0u8; ATTR_HEADER + value.len()];
            put16(&mut record, 0, 1);
            put32(&mut record, 2, value.len() as u32);
            record[ATTR_HEADER..].copy_from_slice(value);
            match self.bt_replace(Special::Attributes, &key, &record) {
                Err(Error::NotFound) => self.bt_insert(Special::Attributes, &key, &record),
                r => r,
            }
        })();
        self.finish(r)
    }

    /// An extended attribute's value.
    pub fn attr_get(&mut self, cnid: u64, name: &[u8]) -> Result<Vec<u8>, Error> {
        if !attr_name_ok(name) {
            return Err(Error::Invalid);
        }
        let r = self.bt_find(Special::Attributes, &attr_key(cnid, name))?;
        if r.len() < ATTR_HEADER || le32(&r, 2) as usize != r.len() - ATTR_HEADER {
            return Err(Error::Corrupt);
        }
        Ok(r[ATTR_HEADER..].to_vec())
    }

    /// Removes an extended attribute.
    pub fn attr_remove(&mut self, cnid: u64, name: &[u8]) -> Result<(), Error> {
        if !attr_name_ok(name) {
            return Err(Error::Invalid);
        }
        self.begin()?;
        let r = self.bt_delete(Special::Attributes, &attr_key(cnid, name));
        self.finish(r)
    }

    /// The names of `cnid`'s attributes, in order.
    pub fn attr_names(&mut self, cnid: u64) -> Result<Vec<Vec<u8>>, Error> {
        let mut out = Vec::new();
        self.bt_walk(Special::Attributes, &be(cnid), |k, _| {
            if k.len() <= 8 || be64(k) != cnid {
                return true;
            }
            out.push(k[8..].to_vec());
            false
        })?;
        Ok(out)
    }

    // ---- Reading and writing ----

    /// Reads up to `buf.len()` bytes at `offset` of a fork; returns how
    /// many (0 at or past the end).
    pub fn read(
        &mut self,
        cnid: u64,
        resource: bool,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, Error> {
        let e = self.get(cnid)?;
        if e.is_dir {
            return Err(Error::IsDir);
        }
        let desc = *e.fork(resource);
        let size = le64(&desc, fork::LENGTH);
        if offset >= size {
            return Ok(0);
        }
        let length = (size - offset).min(buf.len() as u64) as usize;
        let mut block = vec![0u8; BLOCK_SIZE];
        let mut done = 0;
        while done < length {
            let pos = offset + done as u64;
            let inner = (pos % BLOCK_SIZE as u64) as usize;
            let n = (BLOCK_SIZE - inner).min(length - done);
            let (b, _) = self.fork_map(&desc, cnid, resource, pos / BLOCK_SIZE as u64)?;
            self.read_block(b, &mut block)?;
            buf[done..done + n].copy_from_slice(&block[inner..inner + n]);
            done += n;
        }
        Ok(length)
    }

    /// Bytes `offset..offset + length` of a fork get `data` (zeros when
    /// `None`). File blocks from `first_new` on are new: what is not
    /// written in them is zero.
    #[allow(clippy::too_many_arguments)]
    fn put_range(
        &mut self,
        desc: &[u8],
        cnid: u64,
        resource: bool,
        offset: u64,
        data: Option<&[u8]>,
        length: usize,
        first_new: u64,
    ) -> Result<(), Error> {
        let mut block = vec![0u8; BLOCK_SIZE];
        let mut done = 0;
        while done < length {
            let pos = offset + done as u64;
            let index = pos / BLOCK_SIZE as u64;
            let inner = (pos % BLOCK_SIZE as u64) as usize;
            let n = (BLOCK_SIZE - inner).min(length - done);
            let (b, _) = self.fork_map(desc, cnid, resource, index)?;
            if n < BLOCK_SIZE {
                if index >= first_new {
                    block.fill(0);
                } else {
                    self.read_block(b, &mut block)?;
                }
            }
            match data {
                Some(d) => block[inner..inner + n].copy_from_slice(&d[done..done + n]),
                None => block[inner..inner + n].fill(0),
            }
            self.write_data(b, &block)?;
            done += n;
        }
        Ok(())
    }

    /// One transaction's worth of a write; returns the data bytes used (0
    /// while zeroing a gap after the old end).
    fn write_step(
        &mut self,
        cnid: u64,
        resource: bool,
        offset: u64,
        data: Option<&[u8]>,
        now: u64,
    ) -> Result<usize, Error> {
        self.begin()?;
        let r = (|| {
            let mut e = self.get(cnid)?;
            if e.is_dir {
                return Err(Error::IsDir);
            }
            let mut desc = *e.fork(resource);
            let size = le64(&desc, fork::LENGTH);
            let blocks = le64(&desc, fork::BLOCKS);
            let length = data.map_or(0, <[u8]>::len) as u64;
            let (start, source, mut end) = if offset > size {
                (size, None, offset)
            } else {
                (offset, data, offset + length)
            };
            if end - start > CHUNK * BLOCK_SIZE as u64 {
                end = start + CHUNK * BLOCK_SIZE as u64;
            }
            let need = blocks_for(end.max(size));
            if need > blocks {
                self.fork_grow(&mut desc, cnid, resource, need - blocks)?;
            }
            self.put_range(
                &desc,
                cnid,
                resource,
                start,
                source,
                (end - start) as usize,
                blocks,
            )?;
            if end > size {
                put64(&mut desc, fork::LENGTH, end);
            }
            *e.fork_mut(resource) = desc;
            e.times[1] = now;
            self.cat_update(&e)?;
            Ok(if source.is_some() {
                (end - start) as usize
            } else {
                0
            })
        })();
        self.finish(r)
    }

    /// Writes `data` at `offset` of a fork (it grows; a gap after the old
    /// end reads as zeros).
    pub fn write(
        &mut self,
        cnid: u64,
        resource: bool,
        offset: u64,
        data: &[u8],
        now: u64,
    ) -> Result<(), Error> {
        if offset.checked_add(data.len() as u64).is_none() {
            return Err(Error::Invalid);
        }
        let mut done = 0;
        while done < data.len() {
            done += self.write_step(
                cnid,
                resource,
                offset + done as u64,
                Some(&data[done..]),
                now,
            )?;
        }
        Ok(())
    }

    /// Makes a fork `length` bytes long (blocks freed, or zeros added).
    pub fn set_length(
        &mut self,
        cnid: u64,
        resource: bool,
        length: u64,
        now: u64,
    ) -> Result<(), Error> {
        loop {
            let mut e = self.get(cnid)?;
            if e.is_dir {
                return Err(Error::IsDir);
            }
            let mut desc = *e.fork(resource);
            let size = le64(&desc, fork::LENGTH);
            if length == size {
                return Ok(());
            }
            if length > size {
                self.write_step(cnid, resource, length, None, now)?;
                continue;
            }
            // Shorter: the blocks after the new end go back, CHUNK at a
            // time; until the last step the length is what is kept.
            let blocks = le64(&desc, fork::BLOCKS);
            let target = blocks_for(length);
            let keep = if blocks - target > CHUNK {
                blocks - CHUNK
            } else {
                target
            };
            self.begin()?;
            let r = (|| {
                self.fork_shrink(&mut desc, cnid, resource, keep)?;
                put64(
                    &mut desc,
                    fork::LENGTH,
                    if keep == target {
                        length
                    } else {
                        keep * BLOCK_SIZE as u64
                    },
                );
                *e.fork_mut(resource) = desc;
                e.times[1] = now;
                self.cat_update(&e)
            })();
            self.finish(r)?;
        }
    }

    /// Removes a file (its forks' blocks and attributes too) or an empty
    /// folder.
    pub fn remove(&mut self, parent: u64, name: &[u8], now: u64) -> Result<(), Error> {
        let e = self.lookup(parent, name)?;
        if e.is_dir && e.valence != 0 {
            return Err(Error::NotEmpty);
        }
        if e.cnid == crate::format::cnid::ROOT {
            return Err(Error::Invalid);
        }
        if !e.is_dir {
            self.set_length(e.cnid, false, 0, now)?;
            self.set_length(e.cnid, true, 0, now)?;
        }
        for attr in self.attr_names(e.cnid)? {
            self.begin()?;
            let r = self.bt_delete(Special::Attributes, &attr_key(e.cnid, &attr));
            self.finish(r)?;
        }
        self.begin()?;
        let r = self.cat_remove(parent, name, now);
        self.finish(r)
    }
}
