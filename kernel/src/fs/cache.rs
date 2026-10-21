// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The block cache: the most recently used 4 KiB blocks of a device, in
//! memory, write-through (a write goes to the device at once and updates
//! the cached copy). File systems keep their metadata consistent with
//! their own journal, so the cache never holds data the disk lacks.
//!
//! ```text
//!  read(b..b+n): hits → copy from the cache, mark most recent
//!                misses → consecutive ones become one run; all runs go to
//!                   the device in one read_many (no lock held), then are
//!                   inserted, evicting the least recently used when full
//!  write(b): write the device, then update the cached copy if present
//! ```
//!
//! The lock is a `SpinLock`, which disables interrupts while held, so it is
//! never held across device I/O (that may sleep). Two threads missing on
//! the same block both read it; the result is the same. Pure logic over
//! [`BlockDevice`], tested on the host.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use block::{BLOCK_SIZE, BlockDevice, BlockError, check_request};
use frame::sync::SpinLock;

/// Default capacity: 256 blocks (1 MiB).
pub const DEFAULT_BLOCKS: usize = 256;

struct Lru {
    /// Block -> (data, last use).
    blocks: BTreeMap<u64, (Box<[u8; BLOCK_SIZE]>, u64)>,
    /// Last use -> block, oldest first.
    order: BTreeMap<u64, u64>,
    clock: u64,
    capacity: usize,
    hits: u64,
    misses: u64,
}

impl Lru {
    fn touch(&mut self, block: u64) {
        self.clock += 1;
        let now = self.clock;
        if let Some((_, used)) = self.blocks.get_mut(&block) {
            self.order.remove(used);
            *used = now;
            self.order.insert(now, block);
        }
    }

    fn insert(&mut self, block: u64, data: &[u8]) {
        if let Some((cached, _)) = self.blocks.get_mut(&block) {
            cached.copy_from_slice(data);
            self.touch(block);
            return;
        }
        if self.blocks.len() >= self.capacity
            && let Some((_, oldest)) = self.order.pop_first()
        {
            self.blocks.remove(&oldest);
        }
        let mut copy = Box::new([0u8; BLOCK_SIZE]);
        copy.copy_from_slice(data);
        self.clock += 1;
        self.blocks.insert(block, (copy, self.clock));
        self.order.insert(self.clock, block);
    }
}

/// A device with a block cache in front of it.
pub struct CachedDevice<D> {
    device: D,
    lru: SpinLock<Lru>,
}

/// Cache counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheStats {
    /// Blocks cached now.
    pub cached: usize,
    /// Reads served from the cache.
    pub hits: u64,
    /// Reads that went to the device.
    pub misses: u64,
}

impl<D: BlockDevice> CachedDevice<D> {
    /// `device` with a cache of `capacity` blocks.
    pub fn new(device: D, capacity: usize) -> CachedDevice<D> {
        CachedDevice {
            device,
            lru: SpinLock::new(Lru {
                blocks: BTreeMap::new(),
                order: BTreeMap::new(),
                clock: 0,
                capacity: capacity.max(1),
                hits: 0,
                misses: 0,
            }),
        }
    }

    /// The counters.
    pub fn stats(&self) -> CacheStats {
        let lru = self.lru.lock();
        CacheStats {
            cached: lru.blocks.len(),
            hits: lru.hits,
            misses: lru.misses,
        }
    }

    /// Drops every cached block (for example after the device was changed
    /// behind the cache's back).
    pub fn invalidate(&self) {
        let mut lru = self.lru.lock();
        lru.blocks.clear();
        lru.order.clear();
    }

    /// Fills the requests: cached blocks from the cache, the others from
    /// the device, every run of consecutive misses as one request of a
    /// single `read_many`.
    fn fill(&self, requests: &mut [(u64, &mut [u8])]) -> Result<(), BlockError> {
        // The runs missing: (request, first block index in it, blocks).
        let mut runs: Vec<(usize, usize, usize)> = Vec::new();
        {
            let mut lru = self.lru.lock();
            for (r, (block, buf)) in requests.iter_mut().enumerate() {
                let n = buf.len() / BLOCK_SIZE;
                for i in 0..n {
                    let b = *block + i as u64;
                    if let Some((data, _)) = lru.blocks.get(&b) {
                        buf[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE].copy_from_slice(&data[..]);
                        lru.hits += 1;
                        lru.touch(b);
                        continue;
                    }
                    lru.misses += 1;
                    match runs.last_mut() {
                        Some((rr, first, count)) if *rr == r && *first + *count == i => *count += 1,
                        _ => runs.push((r, i, 1)),
                    }
                }
            }
        }
        if runs.is_empty() {
            return Ok(());
        }
        let mut temp: Vec<(u64, Vec<u8>)> = runs
            .iter()
            .map(|&(r, first, count)| (requests[r].0 + first as u64, vec![0u8; count * BLOCK_SIZE]))
            .collect();
        {
            let mut io: Vec<(u64, &mut [u8])> = temp
                .iter_mut()
                .map(|(b, data)| (*b, data.as_mut_slice()))
                .collect();
            self.device.read_many(&mut io)?;
        }
        let mut lru = self.lru.lock();
        for (&(r, first, count), (block, data)) in runs.iter().zip(&temp) {
            for i in 0..count {
                lru.insert(
                    block + i as u64,
                    &data[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE],
                );
            }
            let at = first * BLOCK_SIZE;
            requests[r].1[at..at + data.len()].copy_from_slice(data);
        }
        Ok(())
    }
}

impl<D: BlockDevice> BlockDevice for CachedDevice<D> {
    fn block_count(&self) -> u64 {
        self.device.block_count()
    }

    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check_request(self.block_count(), block, buf.len())?;
        self.fill(&mut [(block, buf)])
    }

    fn read_many(&self, requests: &mut [(u64, &mut [u8])]) -> Result<(), BlockError> {
        for (block, buf) in requests.iter() {
            check_request(self.block_count(), *block, buf.len())?;
        }
        self.fill(requests)
    }

    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        let n = check_request(self.block_count(), block, buf.len())?;
        let result = self.device.write(block, buf);
        let mut lru = self.lru.lock();
        for i in 0..n {
            let at = i as usize * BLOCK_SIZE;
            if result.is_ok() {
                if lru.blocks.contains_key(&(block + i)) {
                    lru.insert(block + i, &buf[at..at + BLOCK_SIZE]);
                }
            } else if let Some((_, used)) = lru.blocks.remove(&(block + i)) {
                // The device may hold either version now: forget it.
                lru.order.remove(&used);
            }
        }
        result
    }

    fn read_only(&self) -> bool {
        self.device.read_only()
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.device.flush()
    }
}

/// A scratch buffer of `n` blocks.
pub fn blocks(n: usize) -> Vec<u8> {
    vec![0u8; n * BLOCK_SIZE]
}

#[cfg(test)]
mod tests {
    use super::*;
    use block::mem::MemDisk;

    /// Counts the device requests (`read_many` calls).
    struct Counting<'a> {
        disk: &'a MemDisk,
        calls: core::cell::Cell<u32>,
        blocks: core::cell::Cell<u64>,
    }

    impl BlockDevice for Counting<'_> {
        fn block_count(&self) -> u64 {
            self.disk.block_count()
        }
        fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            self.disk.read(block, buf)
        }
        fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
            self.disk.write(block, buf)
        }
        fn read_many(&self, requests: &mut [(u64, &mut [u8])]) -> Result<(), BlockError> {
            self.calls.set(self.calls.get() + 1);
            for (_, buf) in requests.iter() {
                self.blocks
                    .set(self.blocks.get() + (buf.len() / BLOCK_SIZE) as u64);
            }
            self.disk.read_many(requests)
        }
    }

    #[test]
    fn misses_go_to_the_device_together() {
        let disk = MemDisk::new(32);
        for b in 0..32u8 {
            disk.write(u64::from(b), &[b; BLOCK_SIZE]).unwrap();
        }
        let counting = Counting {
            disk: &disk,
            calls: core::cell::Cell::new(0),
            blocks: core::cell::Cell::new(0),
        };
        let cache = CachedDevice::new(&counting, 64);
        let mut one = blocks(1);
        cache.read(3, &mut one).unwrap();
        // 0..8 with 3 cached: two runs (0..3, 4..8), one device call.
        let mut eight = blocks(8);
        cache.read(0, &mut eight).unwrap();
        assert_eq!(counting.calls.get(), 2);
        assert_eq!(counting.blocks.get(), 1 + 7);
        for i in 0..8 {
            assert_eq!(eight[i * BLOCK_SIZE], i as u8);
        }
        // Two requests, all cached: no device call.
        let (mut a, mut b) = (blocks(2), blocks(1));
        cache
            .read_many(&mut [(1, &mut a[..]), (7, &mut b[..])])
            .unwrap();
        assert_eq!(counting.calls.get(), 2);
        assert_eq!((a[BLOCK_SIZE], b[0]), (2, 7));
    }

    #[test]
    fn hits_misses_and_eviction() {
        let disk = MemDisk::new(16);
        disk.write(3, &[3u8; BLOCK_SIZE]).unwrap();
        let cache = CachedDevice::new(&disk, 2);
        let mut b = blocks(1);
        cache.read(3, &mut b).unwrap();
        assert_eq!(b[0], 3);
        cache.read(3, &mut b).unwrap();
        assert_eq!(
            cache.stats(),
            CacheStats {
                cached: 1,
                hits: 1,
                misses: 1
            }
        );
        cache.read(4, &mut b).unwrap();
        cache.read(3, &mut b).unwrap(); // 3 is now the most recent
        cache.read(5, &mut b).unwrap(); // evicts 4
        let s = cache.stats();
        assert_eq!((s.cached, s.hits, s.misses), (2, 2, 3));
        cache.read(3, &mut b).unwrap();
        assert_eq!(cache.stats().hits, 3);
        cache.read(4, &mut b).unwrap();
        assert_eq!(cache.stats().misses, 4);
    }

    #[test]
    fn write_through() {
        let disk = MemDisk::new(8);
        let cache = CachedDevice::new(&disk, 4);
        let mut b = blocks(2);
        cache.read(1, &mut b).unwrap();
        cache.write(1, &[9u8; 2 * BLOCK_SIZE]).unwrap();
        // The disk has it at once, and so does the cached block 1.
        let mut direct = blocks(1);
        disk.read(2, &mut direct).unwrap();
        assert_eq!(direct[0], 9);
        cache.read(1, &mut b).unwrap();
        assert!(b.iter().all(|&x| x == 9));
        assert_eq!(
            cache.read(7, &mut b),
            Err(BlockError::OutOfRange),
            "a request past the end"
        );
        cache.invalidate();
        assert_eq!(cache.stats().cached, 0);
    }
}
