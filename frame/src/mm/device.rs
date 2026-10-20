// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Device memory that user space may map: a physical range that is not
//! RAM (the boot framebuffer). Only frame makes these, from ranges it
//! checked, so mapping one cannot expose kernel memory.

/// A page-aligned physical range of device memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRange {
    phys: u64,
    pages: usize,
}

impl DeviceRange {
    /// The pages covering `phys..phys + len`. The caller (in frame) has
    /// checked that the range is device memory.
    pub(crate) fn new(phys: u64, len: u64) -> DeviceRange {
        let start = phys & !0xFFF;
        let end = (phys + len).next_multiple_of(4096);
        DeviceRange {
            phys: start,
            pages: ((end - start) / 4096) as usize,
        }
    }

    /// The first page's physical address.
    pub fn phys(&self) -> u64 {
        self.phys
    }

    /// Number of pages.
    pub fn pages(&self) -> usize {
        self.pages
    }

    /// Size in bytes.
    pub fn size(&self) -> u64 {
        self.pages as u64 * 4096
    }
}
