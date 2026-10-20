// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! DMA buffers: physically contiguous, zeroed frames that a device reads
//! or writes by physical address while the driver reaches them through
//! the HHDM.
//!
//! The device may write the memory at any time, so no Rust reference into
//! it is ever handed out: the driver copies bytes in and out, and reads
//! and writes single fields (descriptors, completion entries) with
//! volatile accesses. The frames go back to the PMM when the buffer is
//! dropped; a driver must stop the device from using a buffer before it
//! drops it.
//!
//! The HHDM maps RAM write-back cached; on x86 DMA is cache coherent
//! (Intel SDM Vol. 3A 12.3 "Methods of Caching Available": snooping), so
//! no flushes are needed. Physical addresses below 4 GiB are not
//! guaranteed; drivers of 32-bit-only devices must check [`DmaBuffer::phys`].

use super::pmm::{self, PhysFrames};
use bootinfo::{HHDM_BASE, PAGE_SIZE};

/// Contiguous memory for a device.
#[derive(Debug)]
pub struct DmaBuffer {
    /// `None` only during drop.
    frames: Option<PhysFrames>,
}

/// An access outside the buffer, or misaligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfRange;

impl DmaBuffer {
    /// `pages` zeroed, physically contiguous pages; `None` when the PMM has
    /// no such run.
    pub fn new(pages: usize) -> Option<DmaBuffer> {
        if pages == 0 {
            return None;
        }
        let frames = pmm::alloc_contiguous(pages)?;
        // SAFETY: the frames were just allocated, so this is their only
        // user, and the HHDM maps them writable.
        unsafe {
            core::ptr::write_bytes(
                (HHDM_BASE + frames.addr()) as *mut u8,
                0,
                pages * PAGE_SIZE as usize,
            )
        };
        Some(DmaBuffer {
            frames: Some(frames),
        })
    }

    fn frames(&self) -> &PhysFrames {
        match &self.frames {
            Some(f) => f,
            None => unreachable!("dma: buffer used during drop"),
        }
    }

    /// Physical address of the first byte (what the device is given).
    pub fn phys(&self) -> u64 {
        self.frames().addr()
    }

    /// Size in bytes.
    pub fn len(&self) -> usize {
        self.frames().count() * PAGE_SIZE as usize
    }

    /// Always false: buffers have at least one page.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The HHDM address of byte `offset`, if `offset..offset + len` is
    /// inside the buffer and `offset` is a multiple of `align`.
    fn at(&self, offset: usize, len: usize, align: usize) -> Result<u64, OutOfRange> {
        let end = offset.checked_add(len).ok_or(OutOfRange)?;
        if end > self.len() || !offset.is_multiple_of(align) {
            return Err(OutOfRange);
        }
        Ok(HHDM_BASE + self.phys() + offset as u64)
    }

    /// Copies bytes at `offset` into `buf`.
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<(), OutOfRange> {
        let src = self.at(offset, buf.len(), 1)?;
        // SAFETY: inside this buffer's frames (checked), mapped by the
        // HHDM; the device may write concurrently, the bytes are copied as
        // plain data and no reference into the buffer exists.
        unsafe { core::ptr::copy_nonoverlapping(src as *const u8, buf.as_mut_ptr(), buf.len()) };
        Ok(())
    }

    /// Copies `data` to `offset`.
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<(), OutOfRange> {
        let dst = self.at(offset, data.len(), 1)?;
        // SAFETY: as in `read`.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), dst as *mut u8, data.len()) };
        Ok(())
    }

    /// Fills the whole buffer with zeros.
    pub fn zero(&self) {
        let dst = HHDM_BASE + self.phys();
        // SAFETY: the whole buffer, mapped by the HHDM; no references.
        unsafe { core::ptr::write_bytes(dst as *mut u8, 0, self.len()) };
    }

    /// Reads the aligned 32-bit value at `offset` (volatile).
    pub fn read_u32(&self, offset: usize) -> Result<u32, OutOfRange> {
        let src = self.at(offset, 4, 4)?;
        // SAFETY: aligned and inside the buffer (checked).
        Ok(unsafe { (src as *const u32).read_volatile() })
    }

    /// Writes the aligned 32-bit value at `offset` (volatile).
    pub fn write_u32(&self, offset: usize, value: u32) -> Result<(), OutOfRange> {
        let dst = self.at(offset, 4, 4)?;
        // SAFETY: aligned and inside the buffer (checked).
        unsafe { (dst as *mut u32).write_volatile(value) };
        Ok(())
    }

    /// Reads the aligned 64-bit value at `offset` (volatile).
    pub fn read_u64(&self, offset: usize) -> Result<u64, OutOfRange> {
        let src = self.at(offset, 8, 8)?;
        // SAFETY: aligned and inside the buffer (checked).
        Ok(unsafe { (src as *const u64).read_volatile() })
    }

    /// Writes the aligned 64-bit value at `offset` (volatile).
    pub fn write_u64(&self, offset: usize, value: u64) -> Result<(), OutOfRange> {
        let dst = self.at(offset, 8, 8)?;
        // SAFETY: aligned and inside the buffer (checked).
        unsafe { (dst as *mut u64).write_volatile(value) };
        Ok(())
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        if let Some(frames) = self.frames.take() {
            pmm::free_contiguous(frames);
        }
    }
}
