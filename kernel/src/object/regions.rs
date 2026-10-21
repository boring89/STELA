// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where `MemoryMap` puts mappings: a first-fit allocator of page-aligned
//! ranges in a process's map area (`abi::layout::MAP_BASE..MAP_END`).
//! Every range is followed by one unused guard page, so running off the
//! end of a mapping faults instead of reaching the next one. Pure logic.

use alloc::collections::BTreeMap;

const PAGE: u64 = 4096;

/// The ranges in use in one address area.
#[derive(Clone, Debug)]
pub struct Regions {
    base: u64,
    end: u64,
    /// Start -> size (without the guard page).
    used: BTreeMap<u64, u64>,
}

impl Regions {
    /// An empty area `base..end` (page aligned).
    pub const fn new(base: u64, end: u64) -> Regions {
        Regions {
            base,
            end,
            used: BTreeMap::new(),
        }
    }

    /// Reserves `size` bytes (rounded up to pages) at the lowest address
    /// that fits, and returns it. `None` if `size` is 0 or nothing fits.
    pub fn alloc(&mut self, size: u64) -> Option<u64> {
        if size == 0 {
            return None;
        }
        let size = size.checked_next_multiple_of(PAGE)?;
        let need = size.checked_add(PAGE)?;
        let mut at = self.base;
        for (&start, &len) in &self.used {
            if start.checked_sub(at)? >= need {
                break;
            }
            at = start + len + PAGE;
        }
        if self.end.checked_sub(at)? < need {
            return None;
        }
        self.used.insert(at, size);
        Some(at)
    }

    /// Releases the range that starts at `start`; returns its size.
    pub fn free(&mut self, start: u64) -> Option<u64> {
        self.used.remove(&start)
    }

    /// The ranges in use: (start, size).
    pub fn ranges(&self) -> alloc::vec::Vec<(u64, u64)> {
        self.used.iter().map(|(&a, &s)| (a, s)).collect()
    }

    /// The size of the range that starts at `start`.
    pub fn size_of(&self, start: u64) -> Option<u64> {
        self.used.get(&start).copied()
    }

    /// Number of ranges in use.
    pub fn len(&self) -> usize {
        self.used.len()
    }

    /// Whether no range is in use.
    pub fn is_empty(&self) -> bool {
        self.used.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fit_with_guard_pages() {
        let mut r = Regions::new(0x1000_0000, 0x1001_0000);
        assert_eq!(r.alloc(0), None);
        let a = r.alloc(1).unwrap();
        assert_eq!(a, 0x1000_0000);
        assert_eq!(r.size_of(a), Some(PAGE));
        let b = r.alloc(2 * PAGE).unwrap();
        assert_eq!(b, a + 2 * PAGE); // a, guard, b
        let c = r.alloc(PAGE).unwrap();
        assert_eq!(c, b + 3 * PAGE);
        // The hole left by a is reused by something that fits.
        assert_eq!(r.free(a), Some(PAGE));
        assert_eq!(r.free(a), None);
        assert_eq!(r.alloc(2 * PAGE), Some(c + 2 * PAGE));
        assert_eq!(r.alloc(PAGE), Some(a));
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn full_area() {
        let mut r = Regions::new(0, 4 * PAGE);
        assert_eq!(r.alloc(3 * PAGE), Some(0));
        assert_eq!(r.alloc(1), None);
        assert_eq!(r.alloc(u64::MAX), None);
        r.free(0);
        assert!(r.is_empty());
        assert_eq!(r.alloc(4 * PAGE), None);
        assert_eq!(r.alloc(3 * PAGE + 1), None);
    }
}
