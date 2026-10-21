// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! USB mass storage, Bulk-Only Transport (P11 K41, `docs/usb.md` §12):
//! every SCSI command is a Command Block Wrapper on the bulk OUT
//! endpoint, an optional data stage, and a Command Status Wrapper on the
//! bulk IN endpoint (USB Mass Storage Class Bulk-Only Transport 1.0 §5).
//!
//! ```text
//!  CBW 31 bytes: "USBC" tag length flags(80 = IN) lun cdb-length cdb[16]
//!  data: length bytes, IN or OUT
//!  CSW 13 bytes: "USBS" tag residue status (0 good, 1 failed, 2 phase error)
//! ```
//!
//! The commands (SCSI SBC/SPC): INQUIRY, TEST UNIT READY, READ CAPACITY
//! (10), READ (10), WRITE (10). The wrappers and commands are pure and
//! tested on the host; the transfers are the xHCI driver's
//! (`xhci::Controller::bot`), which owns the device: a [`UsbDisk`] puts
//! each request in a queue and the USB thread serves it.

use alloc::vec::Vec;

/// The CBW signature `USBC`.
pub const CBW_SIGNATURE: u32 = 0x4342_5355;
/// The CSW signature `USBS`.
pub const CSW_SIGNATURE: u32 = 0x5342_5355;

/// A Command Block Wrapper.
pub fn cbw(tag: u32, length: u32, data_in: bool, cdb: &[u8]) -> [u8; 31] {
    let mut b = [0u8; 31];
    b[0..4].copy_from_slice(&CBW_SIGNATURE.to_le_bytes());
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&length.to_le_bytes());
    b[12] = if data_in { 0x80 } else { 0 };
    b[13] = 0;
    let n = cdb.len().min(16);
    b[14] = n as u8;
    b[15..15 + n].copy_from_slice(&cdb[..n]);
    b
}

/// The status of a Command Status Wrapper for `tag`: 0 good, 1 failed,
/// 2 phase error; `None` when it is not one for that command.
pub fn csw_status(b: &[u8], tag: u32) -> Option<u8> {
    if b.len() < 13 {
        return None;
    }
    let sig = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let t = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    (sig == CSW_SIGNATURE && t == tag).then_some(b[12])
}

/// SCSI commands.
pub mod scsi {
    /// TEST UNIT READY (6 bytes, opcode 00).
    pub const TEST_UNIT_READY: [u8; 6] = [0; 6];

    /// INQUIRY for `length` bytes (opcode 12).
    pub fn inquiry(length: u8) -> [u8; 6] {
        [0x12, 0, 0, 0, length, 0]
    }

    /// READ CAPACITY (10) (opcode 25): 8 bytes back.
    pub const READ_CAPACITY_10: [u8; 10] = [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    /// READ (10) (opcode 28) or WRITE (10) (2A) of `count` blocks at `lba`.
    pub fn rw10(write: bool, lba: u32, count: u16) -> [u8; 10] {
        let l = lba.to_be_bytes();
        let c = count.to_be_bytes();
        [
            if write { 0x2A } else { 0x28 },
            0,
            l[0],
            l[1],
            l[2],
            l[3],
            0,
            c[0],
            c[1],
            0,
        ]
    }

    /// (blocks, block size) from READ CAPACITY (10)'s answer: the last
    /// LBA plus one, and the length of a block (big endian).
    pub fn capacity(b: &[u8]) -> Option<(u64, u32)> {
        if b.len() < 8 {
            return None;
        }
        let last = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        let size = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
        (size >= 512 && size.is_power_of_two() && size <= 4096)
            .then_some((u64::from(last) + 1, size))
    }

    /// The vendor and product from INQUIRY data (bytes 8–15 and 16–31),
    /// trimmed.
    pub fn names(b: &[u8]) -> (alloc::string::String, alloc::string::String) {
        let text = |r: core::ops::Range<usize>| {
            let s: alloc::string::String = b
                .get(r)
                .unwrap_or(&[])
                .iter()
                .map(|&c| {
                    if (0x20..0x7F).contains(&c) {
                        c as char
                    } else {
                        ' '
                    }
                })
                .collect();
            alloc::string::String::from(s.trim())
        };
        (text(8..16), text(16..32))
    }
}

/// Bytes of a block the rest of the kernel uses.
pub const BLOCK: usize = 4096;

/// What a request asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Read one 4 KiB block.
    Read(u64),
    /// Write one 4 KiB block.
    Write(u64),
}

/// A request to the USB thread and its answer.
pub struct Job {
    /// The disk's slot.
    pub slot: u8,
    /// What.
    pub op: Op,
    /// The data: filled by a read, given to a write.
    pub data: frame::sync::SpinLock<Vec<u8>>,
    /// The answer (`true`: done); `None` while it is pending.
    pub result: frame::sync::SpinLock<Option<bool>>,
    /// The thread waiting for it.
    pub waiter: core::sync::atomic::AtomicU64,
}

#[cfg(target_os = "none")]
pub use disk::{JOBS, UsbDisk, take_jobs};

#[cfg(target_os = "none")]
mod disk {
    use super::{BLOCK, Job, Op};
    use alloc::collections::VecDeque;
    use alloc::sync::Arc;
    use alloc::vec;
    use block::{BlockDevice, BlockError};
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use frame::sync::SpinLock;

    /// Requests waiting for the USB thread.
    pub static JOBS: SpinLock<VecDeque<Arc<Job>>> = SpinLock::new(VecDeque::new());

    /// The pending requests, all of them.
    pub fn take_jobs() -> VecDeque<Arc<Job>> {
        core::mem::take(&mut *JOBS.lock())
    }

    /// How long a request may take.
    const TIMEOUT_NS: u64 = 10_000_000_000;

    /// A USB stick or disk as a [`BlockDevice`] (4 KiB blocks).
    pub struct UsbDisk {
        /// Its xHCI slot.
        pub slot: u8,
        /// 4 KiB blocks.
        pub blocks: u64,
        /// Unplugged: every request fails.
        pub gone: AtomicBool,
    }

    impl UsbDisk {
        fn request(
            &self,
            op: Op,
            data: alloc::vec::Vec<u8>,
        ) -> Result<alloc::vec::Vec<u8>, BlockError> {
            if self.gone.load(Ordering::Acquire) {
                return Err(BlockError::Io);
            }
            let job = Arc::new(Job {
                slot: self.slot,
                op,
                data: SpinLock::new(data),
                result: SpinLock::new(None),
                waiter: AtomicU64::new(0),
            });
            JOBS.lock().push_back(Arc::clone(&job));
            crate::usb::xhci::kick();
            let done = crate::sync::wait_for(
                TIMEOUT_NS,
                |me| job.waiter.store(me, Ordering::Release),
                || job.result.lock().is_some(),
            );
            match (*job.result.lock(), done) {
                (Some(true), _) => Ok(core::mem::take(&mut *job.data.lock())),
                (Some(false), _) => Err(BlockError::Io),
                (None, _) => Err(BlockError::Timeout),
            }
        }
    }

    impl BlockDevice for UsbDisk {
        fn block_count(&self) -> u64 {
            self.blocks
        }

        fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            if buf.is_empty() || !buf.len().is_multiple_of(BLOCK) {
                return Err(BlockError::BadBuffer);
            }
            for (i, chunk) in buf.chunks_mut(BLOCK).enumerate() {
                let b = block + i as u64;
                if b >= self.blocks {
                    return Err(BlockError::OutOfRange);
                }
                let data = self.request(Op::Read(b), vec![0u8; BLOCK])?;
                chunk.copy_from_slice(&data[..BLOCK]);
            }
            Ok(())
        }

        fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
            if buf.is_empty() || !buf.len().is_multiple_of(BLOCK) {
                return Err(BlockError::BadBuffer);
            }
            for (i, chunk) in buf.chunks(BLOCK).enumerate() {
                let b = block + i as u64;
                if b >= self.blocks {
                    return Err(BlockError::OutOfRange);
                }
                self.request(Op::Write(b), chunk.to_vec())?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrappers() {
        let b = cbw(7, 4096, true, &scsi::rw10(false, 0x0102_0304, 8));
        assert_eq!(&b[0..4], b"USBC");
        assert_eq!(u32::from_le_bytes([b[4], b[5], b[6], b[7]]), 7);
        assert_eq!(u32::from_le_bytes([b[8], b[9], b[10], b[11]]), 4096);
        assert_eq!((b[12], b[14]), (0x80, 10));
        assert_eq!(&b[15..25], &[0x28, 0, 1, 2, 3, 4, 0, 0, 8, 0]);
        let mut csw = [0u8; 13];
        csw[0..4].copy_from_slice(b"USBS");
        csw[4..8].copy_from_slice(&7u32.to_le_bytes());
        csw[12] = 1;
        assert_eq!(csw_status(&csw, 7), Some(1));
        assert_eq!(csw_status(&csw, 8), None);
        assert_eq!(scsi::rw10(true, 1, 1)[0], 0x2A);
    }

    #[test]
    fn capacity_and_names() {
        assert_eq!(
            scsi::capacity(&[0, 0, 0x7F, 0xFF, 0, 0, 2, 0]),
            Some((0x8000, 512))
        );
        assert_eq!(scsi::capacity(&[0, 0, 0, 1, 0, 0, 0, 100]), None);
        let mut inq = [0u8; 36];
        inq[8..16].copy_from_slice(b"QEMU    ");
        inq[16..32].copy_from_slice(b"QEMU HARDDISK   ");
        let (v, p) = scsi::names(&inq);
        assert_eq!((v.as_str(), p.as_str()), ("QEMU", "QEMU HARDDISK"));
    }
}
