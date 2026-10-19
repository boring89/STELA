// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Synchronisation primitives.

use core::cell::UnsafeCell;

/// A cell for statics that are written only during single-threaded boot
/// (descriptor tables, the TSS) and handed to the hardware by address.
///
/// It gives out raw pointers only; every access is `unsafe` and must argue
/// why no other reference is alive.
pub(crate) struct RacyCell<T>(UnsafeCell<T>);

// SAFETY: users of RacyCell only access it during single-threaded
// initialisation (documented at each use), so no data race can occur.
unsafe impl<T> Sync for RacyCell<T> {}

impl<T> RacyCell<T> {
    /// Creates a cell.
    pub(crate) const fn new(value: T) -> RacyCell<T> {
        RacyCell(UnsafeCell::new(value))
    }

    /// A raw pointer to the contents.
    pub(crate) const fn get(&self) -> *mut T {
        self.0.get()
    }
}
