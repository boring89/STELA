// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Copy-on-write (P13 K6, `docs/linux.md` §11.6): the reference counts of
//! frames that several address spaces map after a `fork`.
//!
//! ```text
//!  fork:      parent PTE ──┐                both read-only + COW bit
//!                          ├──► frame F     counts[F] = 2
//!  child PTE ──────────────┘
//!  a write:   fault → count 2: copy F to G, map G writable, counts[F] = 1
//!                     count 1: map F writable again (nothing to copy)
//!  exit:      pmm::free_frame(F) → only the last reference frees it
//! ```
//!
//! A frame not in the table has one owner (the usual case), so the table
//! only holds frames that are shared right now: a `BTreeMap`, not a count
//! per frame of RAM (the plan's `Vec<u16>` would be 32 MiB of heap on a
//! machine with 64 GiB). Pure logic; the global table is `COUNTS`.

use alloc::collections::BTreeMap;

/// Reference counts of shared frames (by physical address).
#[derive(Debug, Default)]
pub struct RefCounts {
    /// Frames with at least two references.
    shared: BTreeMap<u64, u32>,
}

impl RefCounts {
    /// An empty table.
    pub const fn new() -> RefCounts {
        RefCounts {
            shared: BTreeMap::new(),
        }
    }

    /// One more reference to `frame`.
    pub fn share(&mut self, frame: u64) {
        *self.shared.entry(frame).or_insert(1) += 1;
    }

    /// One reference to `frame` goes away; true if it was the last (the
    /// caller frees the frame).
    pub fn release(&mut self, frame: u64) -> bool {
        match self.shared.get_mut(&frame) {
            Some(n) if *n > 2 => {
                *n -= 1;
                false
            }
            Some(_) => {
                self.shared.remove(&frame);
                false
            }
            None => true,
        }
    }

    /// The references to `frame` (1 if it is not shared).
    pub fn count(&self, frame: u64) -> u32 {
        self.shared.get(&frame).copied().unwrap_or(1)
    }

    /// Number of frames shared now.
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Whether no frame is shared.
    pub fn is_empty(&self) -> bool {
        self.shared.is_empty()
    }
}

/// The table for all address spaces (taken before the PMM's lock).
#[cfg(target_os = "none")]
pub(crate) static COUNTS: crate::sync::SpinLock<RefCounts> =
    crate::sync::SpinLock::new(RefCounts::new());

/// Number of frames shared copy-on-write now (selftest, `mem`).
#[cfg(target_os = "none")]
pub fn shared_frames() -> usize {
    COUNTS.lock().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_references() {
        let mut c = RefCounts::new();
        assert_eq!(c.count(0x1000), 1);
        // A frame with one owner: releasing it frees it.
        assert!(c.release(0x1000));
        // fork: two references; one more fork: three.
        c.share(0x2000);
        assert_eq!(c.count(0x2000), 2);
        c.share(0x2000);
        assert_eq!(c.count(0x2000), 3);
        assert_eq!(c.len(), 1);
        assert!(!c.release(0x2000));
        assert_eq!(c.count(0x2000), 2);
        // Down to one owner: out of the table, not freed.
        assert!(!c.release(0x2000));
        assert_eq!(c.count(0x2000), 1);
        assert!(c.is_empty());
        // The last owner frees it.
        assert!(c.release(0x2000));
    }
}
