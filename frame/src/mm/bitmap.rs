// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A bitmap over borrowed storage: one bit per item, 1 = used, 0 = free.
//! Pure logic, tested on the host.

/// Bits per storage word.
const BITS: usize = u64::BITS as usize;

/// A bitmap of `len` bits stored in `words` (bit `i` is bit `i % 64` of word
/// `i / 64`).
pub struct Bitmap<'a> {
    words: &'a mut [u64],
    len: usize,
}

impl<'a> Bitmap<'a> {
    /// Number of `u64` words needed for `len` bits.
    pub const fn words_for(len: usize) -> usize {
        len.div_ceil(BITS)
    }

    /// Wraps `words` as a bitmap of `len` bits, all set (used). Panics if
    /// `words` is too short.
    pub fn new_full(words: &'a mut [u64], len: usize) -> Bitmap<'a> {
        assert!(
            words.len() >= Bitmap::words_for(len),
            "bitmap storage too small"
        );
        words.fill(u64::MAX);
        Bitmap { words, len }
    }

    /// Number of bits.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the bitmap has no bits.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether bit `index` is set. Bits past the end read as set.
    pub fn is_set(&self, index: usize) -> bool {
        index >= self.len || self.words[index / BITS] & (1 << (index % BITS)) != 0
    }

    /// Sets bit `index`; ignored past the end.
    pub fn set(&mut self, index: usize) {
        if index < self.len {
            self.words[index / BITS] |= 1 << (index % BITS);
        }
    }

    /// Clears bit `index`; ignored past the end.
    pub fn clear(&mut self, index: usize) {
        if index < self.len {
            self.words[index / BITS] &= !(1 << (index % BITS));
        }
    }

    /// Sets bits `start..end` (clamped to the length).
    pub fn set_range(&mut self, start: usize, end: usize) {
        for index in start..end.min(self.len) {
            self.set(index);
        }
    }

    /// Clears bits `start..end` (clamped to the length).
    pub fn clear_range(&mut self, start: usize, end: usize) {
        for index in start..end.min(self.len) {
            self.clear(index);
        }
    }

    /// Number of clear bits.
    pub fn count_clear(&self) -> usize {
        (0..self.len).filter(|&i| !self.is_set(i)).count()
    }

    /// The first clear bit at or after `from`, skipping full words quickly.
    pub fn find_clear(&self, from: usize) -> Option<usize> {
        let mut index = from;
        while index < self.len {
            let word = self.words[index / BITS] | ((1u64 << (index % BITS)) - 1);
            if word == u64::MAX {
                index = (index / BITS + 1) * BITS;
                continue;
            }
            let found = (index / BITS) * BITS + word.trailing_ones() as usize;
            return (found < self.len).then_some(found);
        }
        None
    }

    /// The first run of `count` clear bits, starting at a multiple of
    /// `align` (which must be non-zero). First fit, linear time.
    pub fn find_clear_run(&self, count: usize, align: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let mut start = 0;
        while start + count <= self.len {
            start = self.find_clear(start)?.next_multiple_of(align);
            if start + count > self.len {
                return None;
            }
            match (start..start + count).find(|&i| self.is_set(i)) {
                None => return Some(start),
                Some(used) => start = used + 1,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_full_and_tracks_bits() {
        let mut words = [0; 3];
        let mut map = Bitmap::new_full(&mut words, 130);
        assert_eq!(map.len(), 130);
        assert_eq!(map.count_clear(), 0);
        assert_eq!(map.find_clear(0), None);
        map.clear(129);
        assert!(!map.is_set(129));
        assert_eq!(map.find_clear(0), Some(129));
        map.set(129);
        assert!(map.is_set(129));
        // Out of range: reads as used, writes ignored.
        map.clear(130);
        assert!(map.is_set(130));
        assert_eq!(map.count_clear(), 0);
    }

    #[test]
    fn ranges_and_search() {
        let mut words = [0; 4];
        let mut map = Bitmap::new_full(&mut words, 256);
        map.clear_range(10, 20);
        map.clear_range(70, 200);
        assert_eq!(map.count_clear(), 10 + 130);
        assert_eq!(map.find_clear(0), Some(10));
        assert_eq!(map.find_clear(15), Some(15));
        assert_eq!(map.find_clear(20), Some(70));
        map.set_range(70, 128);
        assert_eq!(map.find_clear(20), Some(128));
        assert_eq!(map.find_clear(200), None);
    }

    #[test]
    fn find_clear_ignores_padding_bits() {
        let mut words = [0; 1];
        let map = Bitmap::new_full(&mut words, 10);
        map.words[0] = 0b11_1111_1111; // bits 10..64 clear but out of range
        assert_eq!(map.find_clear(0), None);
    }

    #[test]
    fn runs() {
        let mut words = [0; 2];
        let mut map = Bitmap::new_full(&mut words, 128);
        map.clear_range(3, 6); // 3 free
        map.clear_range(8, 20); // 12 free
        assert_eq!(map.find_clear_run(3, 1), Some(3));
        assert_eq!(map.find_clear_run(4, 1), Some(8));
        assert_eq!(map.find_clear_run(4, 16), Some(16));
        assert_eq!(map.find_clear_run(5, 16), None);
        assert_eq!(map.find_clear_run(13, 1), None);
        assert_eq!(map.find_clear_run(0, 1), None);
        map.clear_range(120, 128);
        assert_eq!(map.find_clear_run(8, 8), Some(8));
        assert_eq!(map.find_clear_run(8, 40), Some(120));
        assert_eq!(map.find_clear_run(9, 40), None);
    }
}
