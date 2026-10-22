// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A partition: a window of blocks on another device.

use crate::{BlockDevice, BlockError, check_request};

/// Blocks `start..start + count` of `D`, seen as a device of its own.
#[derive(Debug)]
pub struct Partition<D> {
    device: D,
    start: u64,
    count: u64,
    read_only: bool,
}

impl<D: BlockDevice> Partition<D> {
    /// The window `start..start + count` of `device`; `None` if it does not
    /// fit.
    pub fn new(device: D, start: u64, count: u64, read_only: bool) -> Option<Partition<D>> {
        let end = start.checked_add(count)?;
        (end <= device.block_count()).then_some(Partition {
            device,
            start,
            count,
            read_only,
        })
    }

    /// The first block on the underlying device.
    pub fn start(&self) -> u64 {
        self.start
    }
}

impl<D: BlockDevice> BlockDevice for Partition<D> {
    fn block_count(&self) -> u64 {
        self.count
    }

    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check_request(self.count, block, buf.len())?;
        self.device.read(self.start + block, buf)
    }

    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        if self.read_only() {
            return Err(BlockError::ReadOnly);
        }
        check_request(self.count, block, buf.len())?;
        self.device.write(self.start + block, buf)
    }

    fn read_only(&self) -> bool {
        self.read_only || self.device.read_only()
    }

    fn sector_size(&self) -> u32 {
        self.device.sector_size()
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.device.flush()
    }

    fn read_many(&self, requests: &mut [(u64, &mut [u8])]) -> Result<(), BlockError> {
        for (block, buf) in requests.iter_mut() {
            check_request(self.count, *block, buf.len())?;
            *block += self.start;
        }
        let r = self.device.read_many(requests);
        for (block, _) in requests.iter_mut() {
            *block -= self.start;
        }
        r
    }

    fn write_many(&self, requests: &[(u64, &[u8])]) -> Result<(), BlockError> {
        if self.read_only() {
            return Err(BlockError::ReadOnly);
        }
        let mut moved = alloc::vec::Vec::with_capacity(requests.len());
        for &(block, buf) in requests {
            check_request(self.count, block, buf.len())?;
            moved.push((self.start + block, buf));
        }
        self.device.write_many(&moved)
    }
}
