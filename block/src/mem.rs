// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A disk in memory, for tests: file system tests run on it, and it can
//! simulate a power cut ([`MemDisk::cut_power_after`]: writes after the
//! given number of blocks are silently lost).

use crate::{BLOCK_SIZE, BlockDevice, BlockError, check_request};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

/// An in-memory disk (single-threaded: not `Sync`).
#[derive(Debug)]
pub struct MemDisk {
    data: RefCell<Vec<u8>>,
    /// Blocks that may still be written before the "power cut"; `None`:
    /// no cut.
    budget: Cell<Option<u64>>,
    /// Blocks written so far.
    written: Cell<u64>,
}

impl MemDisk {
    /// A zeroed disk of `blocks` blocks.
    pub fn new(blocks: u64) -> MemDisk {
        MemDisk::from_bytes(vec![0; blocks as usize * BLOCK_SIZE])
    }

    /// A disk holding `bytes` (rounded up to whole blocks with zeros).
    pub fn from_bytes(mut bytes: Vec<u8>) -> MemDisk {
        bytes.resize(bytes.len().next_multiple_of(BLOCK_SIZE), 0);
        MemDisk {
            data: RefCell::new(bytes),
            budget: Cell::new(None),
            written: Cell::new(0),
        }
    }

    /// A copy of the contents.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.borrow().clone()
    }

    /// After `blocks` more blocks are written, later writes are dropped
    /// (they report success, as a disk losing power mid-way would not get
    /// to report anything).
    pub fn cut_power_after(&self, blocks: u64) {
        self.budget.set(Some(blocks));
    }

    /// Restores power.
    pub fn restore_power(&self) {
        self.budget.set(None);
    }

    /// Blocks written since creation.
    pub fn blocks_written(&self) -> u64 {
        self.written.get()
    }
}

impl BlockDevice for MemDisk {
    fn block_count(&self) -> u64 {
        (self.data.borrow().len() / BLOCK_SIZE) as u64
    }

    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check_request(self.block_count(), block, buf.len())?;
        let at = block as usize * BLOCK_SIZE;
        buf.copy_from_slice(&self.data.borrow()[at..at + buf.len()]);
        Ok(())
    }

    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        let n = check_request(self.block_count(), block, buf.len())?;
        let mut data = self.data.borrow_mut();
        for i in 0..n {
            if let Some(left) = self.budget.get() {
                if left == 0 {
                    continue;
                }
                self.budget.set(Some(left - 1));
            }
            let at = (block + i) as usize * BLOCK_SIZE;
            let src = i as usize * BLOCK_SIZE;
            data[at..at + BLOCK_SIZE].copy_from_slice(&buf[src..src + BLOCK_SIZE]);
            self.written.set(self.written.get() + 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Partition;

    #[test]
    fn read_write_and_power_cut() {
        let disk = MemDisk::new(8);
        disk.write(1, &[7u8; 2 * BLOCK_SIZE]).unwrap();
        let mut b = vec![0u8; BLOCK_SIZE];
        disk.read(2, &mut b).unwrap();
        assert!(b.iter().all(|&x| x == 7));
        disk.cut_power_after(1);
        disk.write(4, &[9u8; 2 * BLOCK_SIZE]).unwrap();
        disk.read(5, &mut b).unwrap();
        assert!(b.iter().all(|&x| x == 0), "write after the cut was kept");
        disk.read(4, &mut b).unwrap();
        assert!(b.iter().all(|&x| x == 9));
        assert_eq!(disk.blocks_written(), 3);
    }

    #[test]
    fn partitions() {
        let disk = MemDisk::new(10);
        let p = Partition::new(&disk, 2, 5, false).unwrap();
        assert_eq!(p.block_count(), 5);
        p.write(4, &[3u8; BLOCK_SIZE]).unwrap();
        assert_eq!(p.write(5, &[3u8; BLOCK_SIZE]), Err(BlockError::OutOfRange));
        let mut b = vec![0u8; BLOCK_SIZE];
        disk.read(6, &mut b).unwrap();
        assert_eq!(b[0], 3);
        assert!(Partition::new(&disk, 8, 3, false).is_none());
        let ro = Partition::new(&disk, 0, 1, true).unwrap();
        assert_eq!(ro.write(0, &b), Err(BlockError::ReadOnly));
    }
}
