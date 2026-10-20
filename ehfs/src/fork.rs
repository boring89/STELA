// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Forks (68k_HAL `docs/ehfs.md` section 4.3): runs of blocks. The first
//! 8 runs are in the fork descriptor; the others in the extents tree, in
//! records of 8 runs keyed by (CNID big endian, fork (0 data, 1
//! resource), the file block where the record's first run starts, big
//! endian). A fork's runs cover its file blocks in order without gaps.

use crate::Error;
use crate::format::*;
use crate::volume::Volume;
use alloc::vec::Vec;
use block::BlockDevice;

const KEY_LENGTH: usize = 17;
const RECORD_LENGTH: usize = INLINE_EXTENTS * 16;

fn make_key(cnid: u64, resource: bool, start: u64) -> [u8; KEY_LENGTH] {
    let mut k = [0u8; KEY_LENGTH];
    k[..8].copy_from_slice(&cnid.to_be_bytes());
    k[8] = u8::from(resource);
    k[9..].copy_from_slice(&start.to_be_bytes());
    k
}

fn run(runs: &[u8], i: usize) -> (u64, u64) {
    (le64(runs, 16 * i), le64(runs, 16 * i + 8))
}

fn set_run(runs: &mut [u8], i: usize, start: u64, count: u64) {
    put64(runs, 16 * i, start);
    put64(runs, 16 * i + 8, count);
}

fn last_used(runs: &[u8]) -> Option<usize> {
    (0..INLINE_EXTENTS)
        .take_while(|&i| run(runs, i).1 != 0)
        .last()
}

impl<D: BlockDevice> Volume<D> {
    /// The extents record of this fork with the greatest start <= `index`:
    /// (start, runs).
    fn find_record(
        &mut self,
        cnid: u64,
        resource: bool,
        index: u64,
    ) -> Result<(u64, Vec<u8>), Error> {
        let key = make_key(cnid, resource, index);
        let (found, value) = self.bt_find_le(Special::Extents, &key)?;
        if found.len() != KEY_LENGTH || found[..9] != key[..9] {
            return Err(Error::NotFound);
        }
        if value.len() != RECORD_LENGTH {
            return Err(Error::Corrupt);
        }
        Ok((be64(&found[9..]), value))
    }

    /// The device block of file block `index`, and how many file blocks
    /// from there on are contiguous.
    pub(crate) fn fork_map(
        &mut self,
        desc: &[u8],
        cnid: u64,
        resource: bool,
        index: u64,
    ) -> Result<(u64, u64), Error> {
        if index >= le64(desc, fork::BLOCKS) {
            return Err(Error::Invalid);
        }
        let runs = &desc[fork::EXTENT..];
        let mut covered = 0;
        for i in 0..INLINE_EXTENTS {
            let (s, c) = run(runs, i);
            if c == 0 {
                break;
            }
            if index < covered + c {
                return Ok((s + (index - covered), c - (index - covered)));
            }
            covered += c;
        }
        let (start, record) = match self.find_record(cnid, resource, index) {
            Ok(r) => r,
            Err(Error::NotFound) => return Err(Error::Corrupt),
            Err(e) => return Err(e),
        };
        let mut covered = start;
        for i in 0..INLINE_EXTENTS {
            let (s, c) = run(&record, i);
            if c == 0 {
                break;
            }
            if index < covered + c {
                return Ok((s + (index - covered), c - (index - covered)));
            }
            covered += c;
        }
        Err(Error::Corrupt)
    }

    /// Gives the fork `more` blocks after its last one (near it if
    /// possible).
    pub(crate) fn fork_grow(
        &mut self,
        desc: &mut [u8],
        cnid: u64,
        resource: bool,
        mut more: u64,
    ) -> Result<(), Error> {
        while more > 0 {
            let (record_start, mut record, have_record) =
                match self.find_record(cnid, resource, u64::MAX) {
                    Ok((s, r)) => (s, r, true),
                    Err(Error::NotFound) => (0, Vec::new(), false),
                    Err(e) => return Err(e),
                };
            let j = if have_record {
                match last_used(&record) {
                    Some(j) => Some(j),
                    None => return Err(Error::Corrupt),
                }
            } else {
                last_used(&desc[fork::EXTENT..])
            };
            let runs: &[u8] = if have_record {
                &record
            } else {
                &desc[fork::EXTENT..]
            };
            let end = match j {
                Some(j) => {
                    let (s, c) = run(runs, j);
                    s + c
                }
                None => self.alloc_hint,
            };
            let (start, count) = self.alloc(end, more)?;
            let mut write_record = have_record;
            {
                let runs: &mut [u8] = if have_record {
                    &mut record
                } else {
                    &mut desc[fork::EXTENT..]
                };
                match j {
                    Some(j) if start == end => {
                        let (s, c) = run(runs, j);
                        set_run(runs, j, s, c + count);
                    }
                    Some(j) if j + 1 < INLINE_EXTENTS => set_run(runs, j + 1, start, count),
                    None => set_run(runs, 0, start, count),
                    Some(_) => {
                        // A new record, starting at the file block this run
                        // will be.
                        let mut fresh = [0u8; RECORD_LENGTH];
                        set_run(&mut fresh, 0, start, count);
                        let key = make_key(cnid, resource, le64(desc, fork::BLOCKS));
                        self.bt_insert(Special::Extents, &key, &fresh)?;
                        write_record = false;
                    }
                }
            }
            if write_record {
                let key = make_key(cnid, resource, record_start);
                self.bt_replace(Special::Extents, &key, &record)?;
            }
            let blocks = le64(desc, fork::BLOCKS) + count;
            put64(desc, fork::BLOCKS, blocks);
            more -= count;
        }
        Ok(())
    }

    /// Cuts runs (file blocks from `position` on) to `keep` file blocks;
    /// returns whether any run is left.
    fn cut(&mut self, runs: &mut [u8], mut position: u64, keep: u64) -> Result<bool, Error> {
        let mut left = false;
        for i in 0..INLINE_EXTENTS {
            let (s, c) = run(runs, i);
            if c == 0 {
                break;
            }
            if position >= keep {
                self.free(s, c)?;
                set_run(runs, i, 0, 0);
            } else if position + c > keep {
                self.free(s + (keep - position), position + c - keep)?;
                set_run(runs, i, s, keep - position);
                left = true;
            } else {
                left = true;
            }
            position += c;
        }
        Ok(left)
    }

    /// Keeps the first `keep` blocks of the fork and frees the rest (with
    /// the extents records they used).
    pub(crate) fn fork_shrink(
        &mut self,
        desc: &mut [u8],
        cnid: u64,
        resource: bool,
        keep: u64,
    ) -> Result<(), Error> {
        if keep >= le64(desc, fork::BLOCKS) {
            return Ok(());
        }
        loop {
            let (start, mut record) = match self.find_record(cnid, resource, u64::MAX) {
                Ok(r) => r,
                Err(Error::NotFound) => break,
                Err(e) => return Err(e),
            };
            if start < keep {
                let sum: u64 = (0..INLINE_EXTENTS).map(|i| run(&record, i).1).sum();
                if start + sum <= keep {
                    break;
                }
            }
            let left = self.cut(&mut record, start, keep)?;
            let key = make_key(cnid, resource, start);
            if left {
                self.bt_replace(Special::Extents, &key, &record)?;
                break;
            }
            self.bt_delete(Special::Extents, &key)?;
        }
        self.cut(&mut desc[fork::EXTENT..], 0, keep)?;
        put64(desc, fork::BLOCKS, keep);
        Ok(())
    }

    /// Every run of the fork, and how many extents records it has
    /// (`Corrupt` if the runs do not add up to its blocks).
    pub(crate) fn fork_runs(
        &mut self,
        desc: &[u8],
        cnid: u64,
        resource: bool,
    ) -> Result<(Vec<(u64, u64)>, u64), Error> {
        let mut runs = Vec::new();
        let mut position = 0;
        let mut ended = false;
        for i in 0..INLINE_EXTENTS {
            let (s, c) = run(&desc[fork::EXTENT..], i);
            if c == 0 {
                ended = true;
                continue;
            }
            if ended {
                return Err(Error::Corrupt);
            }
            runs.push((s, c));
            position += c;
        }
        let mut key = make_key(cnid, resource, 0).to_vec();
        let mut records = 0;
        let mut inclusive = true;
        loop {
            let (found, record) = match self.bt_next(Special::Extents, &key, inclusive) {
                Ok(r) => r,
                Err(Error::NotFound) => break,
                Err(e) => return Err(e),
            };
            inclusive = false;
            if found.len() != KEY_LENGTH || found[..9] != key[..9] {
                break;
            }
            if record.len() != RECORD_LENGTH
                || be64(&found[9..]) != position
                || run(&record, 0).1 == 0
            {
                return Err(Error::Corrupt);
            }
            for i in 0..INLINE_EXTENTS {
                let (s, c) = run(&record, i);
                if c == 0 {
                    break;
                }
                runs.push((s, c));
                position += c;
            }
            records += 1;
            key = found;
        }
        if position == le64(desc, fork::BLOCKS) {
            Ok((runs, records))
        } else {
            Err(Error::Corrupt)
        }
    }
}
