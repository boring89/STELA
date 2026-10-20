// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Mac's picture and the window: the model draws the whole built-in
//! screen every time, so the rows that really changed are found by
//! comparing with the picture shown last, and only those are copied to
//! the window and damaged.

use alloc::vec;
use alloc::vec::Vec;

/// The picture last shown and the bands of rows that changed since.
pub struct Screen {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    shown: Vec<u32>,
}

impl Screen {
    /// A screen of `width` x `height` whose first picture counts as
    /// changed everywhere.
    pub fn new(width: usize, height: usize) -> Screen {
        Screen {
            width,
            height,
            shown: vec![u32::MAX; width * height],
        }
    }

    /// Compares `picture` (`width * height` pixels) with the one shown
    /// last, remembers it, and returns the changed rows as bands
    /// `(first row, row count)`, top to bottom. Bands closer than `gap`
    /// rows are merged (fewer, larger damage messages).
    pub fn update(&mut self, picture: &[u32], gap: usize) -> Vec<(usize, usize)> {
        let mut bands: Vec<(usize, usize)> = Vec::new();
        let w = self.width;
        for y in 0..self.height.min(picture.len() / w.max(1)) {
            let row = &picture[y * w..(y + 1) * w];
            let old = &mut self.shown[y * w..(y + 1) * w];
            if row == old {
                continue;
            }
            old.copy_from_slice(row);
            match bands.last_mut() {
                Some((first, count)) if *first + *count + gap >= y => *count = y + 1 - *first,
                _ => bands.push((y, 1)),
            }
        }
        bands
    }
}

/// Seconds between 1904-01-01 (the Mac's epoch) and 1970-01-01 (Unix).
pub const MAC_EPOCH_OFFSET: u64 = 2_082_844_800;

/// The Mac's clock (seconds since 1904, wrapping as the 32-bit clock
/// chip does) for a wall clock in nanoseconds since 1970.
pub fn mac_seconds(unix_ns: u64) -> u32 {
    (unix_ns / 1_000_000_000 + MAC_EPOCH_OFFSET) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_picture_changes_everything() {
        let mut s = Screen::new(4, 3);
        assert_eq!(s.update(&[0; 12], 0), vec![(0, 3)]);
        assert!(s.update(&[0; 12], 0).is_empty());
    }

    #[test]
    fn changed_rows_form_bands() {
        let mut s = Screen::new(2, 6);
        s.update(&[0; 12], 0);
        let mut p = [0u32; 12];
        p[0] = 1; // row 0
        p[5] = 1; // row 2
        p[7] = 1; // row 3
        assert_eq!(s.update(&p, 0), vec![(0, 1), (2, 2)]);
        p[0] = 2;
        p[6] = 2; // rows 0 and 3, gap of 2 rows
        assert_eq!(s.update(&p, 1), vec![(0, 1), (3, 1)]);
        p[0] = 3;
        p[6] = 3;
        assert_eq!(s.update(&p, 2), vec![(0, 4)]);
    }

    #[test]
    fn short_picture_is_safe() {
        let mut s = Screen::new(4, 4);
        assert_eq!(s.update(&[7; 8], 0), vec![(0, 2)]);
    }

    #[test]
    fn mac_clock() {
        assert_eq!(mac_seconds(0), 2_082_844_800);
        // 2026-10-20 00:00 UTC = 1792454400 s since 1970.
        assert_eq!(
            mac_seconds(1_792_454_400 * 1_000_000_000),
            (1_792_454_400u64 + 2_082_844_800) as u32
        );
    }
}
