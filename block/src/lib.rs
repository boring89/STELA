// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Block devices (`docs/storage.md`): the [`BlockDevice`] trait that disk
//! drivers implement and file systems use, in 4 KiB blocks.
//!
//! The trait lives in this small crate so that the kernel, the drivers and
//! the file systems (and the host tools) can all depend on it without
//! depending on each other. Pure logic, `no_std`, tested on the host.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod crc32;
pub mod gpt;
pub mod mem;
pub mod partition;

pub use partition::Partition;

use alloc::sync::Arc;

/// The block size of every [`BlockDevice`]: 4 KiB. Drivers convert to
/// their sector size (512 or 4096).
pub const BLOCK_SIZE: usize = 4096;

/// Why a block operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockError {
    /// The blocks are past the end of the device.
    OutOfRange,
    /// The buffer is not a whole number of blocks (or is empty).
    BadBuffer,
    /// The device is read-only.
    ReadOnly,
    /// The device reported an error.
    Io,
    /// The device did not answer in time.
    Timeout,
    /// No memory for the request.
    OutOfMemory,
}

/// A disk, partition or image, read and written in whole 4 KiB blocks.
/// Operations are synchronous: they return when the data is on the device
/// (written) or in the buffer (read).
pub trait BlockDevice {
    /// Number of 4 KiB blocks.
    fn block_count(&self) -> u64;

    /// Reads `buf.len() / 4096` blocks starting at `block`.
    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Writes `buf.len() / 4096` blocks starting at `block`.
    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError>;

    /// Whether writes are refused.
    fn read_only(&self) -> bool {
        false
    }

    /// Makes earlier writes durable (a cache flush on the device).
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }

    /// Reads several runs of blocks: each request is (first block, buffer)
    /// as for [`BlockDevice::read`]. Drivers that can have many commands
    /// in flight (AHCI, NVMe) issue them together; the default reads one
    /// after the other (P11).
    fn read_many(&self, requests: &mut [(u64, &mut [u8])]) -> Result<(), BlockError> {
        for (block, buf) in requests.iter_mut() {
            self.read(*block, buf)?;
        }
        Ok(())
    }

    /// Writes several runs of blocks, like [`BlockDevice::read_many`].
    fn write_many(&self, requests: &[(u64, &[u8])]) -> Result<(), BlockError> {
        for (block, buf) in requests {
            self.write(*block, buf)?;
        }
        Ok(())
    }
}

impl<T: BlockDevice + ?Sized> BlockDevice for Arc<T> {
    fn block_count(&self) -> u64 {
        (**self).block_count()
    }

    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        (**self).read(block, buf)
    }

    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        (**self).write(block, buf)
    }

    fn read_only(&self) -> bool {
        (**self).read_only()
    }

    fn flush(&self) -> Result<(), BlockError> {
        (**self).flush()
    }

    fn read_many(&self, requests: &mut [(u64, &mut [u8])]) -> Result<(), BlockError> {
        (**self).read_many(requests)
    }

    fn write_many(&self, requests: &[(u64, &[u8])]) -> Result<(), BlockError> {
        (**self).write_many(requests)
    }
}

impl<T: BlockDevice + ?Sized> BlockDevice for &T {
    fn block_count(&self) -> u64 {
        (**self).block_count()
    }

    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        (**self).read(block, buf)
    }

    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        (**self).write(block, buf)
    }

    fn read_only(&self) -> bool {
        (**self).read_only()
    }

    fn flush(&self) -> Result<(), BlockError> {
        (**self).flush()
    }

    fn read_many(&self, requests: &mut [(u64, &mut [u8])]) -> Result<(), BlockError> {
        (**self).read_many(requests)
    }

    fn write_many(&self, requests: &[(u64, &[u8])]) -> Result<(), BlockError> {
        (**self).write_many(requests)
    }
}

/// Checks a request: `buf_len` is a non-zero multiple of the block size and
/// `block..block + buf_len / 4096` is inside `count` blocks. Returns the
/// number of blocks.
pub fn check_request(count: u64, block: u64, buf_len: usize) -> Result<u64, BlockError> {
    if buf_len == 0 || !buf_len.is_multiple_of(BLOCK_SIZE) {
        return Err(BlockError::BadBuffer);
    }
    let n = (buf_len / BLOCK_SIZE) as u64;
    match block.checked_add(n) {
        Some(end) if end <= count => Ok(n),
        _ => Err(BlockError::OutOfRange),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests() {
        assert_eq!(check_request(10, 0, 4096), Ok(1));
        assert_eq!(check_request(10, 8, 8192), Ok(2));
        assert_eq!(check_request(10, 9, 8192), Err(BlockError::OutOfRange));
        assert_eq!(
            check_request(10, u64::MAX, 4096),
            Err(BlockError::OutOfRange)
        );
        assert_eq!(check_request(10, 0, 0), Err(BlockError::BadBuffer));
        assert_eq!(check_request(10, 0, 512), Err(BlockError::BadBuffer));
    }

    #[test]
    fn many_at_once() {
        use crate::mem::MemDisk;
        let disk = MemDisk::new(16);
        let part = Partition::new(&disk, 4, 8, false).unwrap();
        let (a, b) = ([1u8; 4096], [2u8; 8192]);
        part.write_many(&[(0, &a), (5, &b)]).unwrap();
        let mut x = [0u8; 4096];
        disk.read(4, &mut x).unwrap();
        assert_eq!(x[0], 1);
        disk.read(10, &mut x).unwrap();
        assert_eq!(x[0], 2);
        let (mut c, mut d) = ([0u8; 4096], [0u8; 8192]);
        part.read_many(&mut [(5, &mut d), (0, &mut c)]).unwrap();
        assert_eq!((c[0], d[8191]), (1, 2));
        assert_eq!(
            part.read_many(&mut [(7, &mut d)]),
            Err(BlockError::OutOfRange)
        );
    }
}
