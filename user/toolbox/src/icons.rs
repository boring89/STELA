// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! STELA's icons (`docs/identity.md`): the colour and 1-bit images from
//! `assets/`, turned into Rust data by `tools/img_to_rs.py` (`make
//! assets`). **The current images are drafts**, to be replaced by the
//! user's own artwork; only the files in `assets/` change.

use crate::draw::{Canvas, Color, color};

/// One icon at one size.
pub struct Icon {
    /// Its name (`stela`, `kibotos`...).
    pub name: &'static str,
    /// Width and height in pixels.
    pub size: u32,
    /// Colours `0x00RRGGBB`; index 0 is transparent.
    pub palette: &'static [u32],
    /// The colour image, row by row, as (count, palette index) pairs.
    pub runs: &'static [u8],
    /// The 1-bit image: rows of `size.div_ceil(8)` bytes, MSB first.
    pub mono: &'static [u8],
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/generated/icons.rs"
));

/// The icon `name` at `size`, if there is one.
pub fn find(name: &str, size: u32) -> Option<&'static Icon> {
    ALL.iter()
        .copied()
        .find(|i| i.name == name && i.size == size)
}

/// The icon size for a screen `width` pixels wide: 64 from 1920 up, else
/// 32 (as the interface font, `draw::font_for`).
pub fn size_for(width: u32) -> u32 {
    if width >= 1920 { 64 } else { 32 }
}

impl Canvas<'_> {
    /// Draws `icon` with its top-left corner at (x, y); `selected`
    /// darkens it (as Classic Mac OS showed a selected icon).
    pub fn icon(&mut self, x: i32, y: i32, icon: &Icon, selected: bool) {
        // P13 K13: at scale 2 the image twice as large, pixel for pixel.
        if self.scale() == 2
            && let Some(big) = find(icon.name, icon.size * 2)
        {
            self.icon_phys(x * 2, y * 2, big, selected);
            return;
        }
        let size = icon.size as i32;
        let mut at = 0i32;
        for pair in icon.runs.as_chunks::<2>().0 {
            let (count, index) = (i32::from(pair[0]), usize::from(pair[1]));
            if index != 0 {
                let c: Color = icon.palette.get(index).copied().unwrap_or(color::BLACK);
                let c = if selected { darken(c) } else { c };
                for k in at..at + count {
                    self.pixel(x + k % size, y + k / size, c);
                }
            }
            at += count;
        }
    }

    /// `icon` in physical pixels.
    fn icon_phys(&mut self, x: i32, y: i32, icon: &Icon, selected: bool) {
        let size = icon.size as i32;
        let mut row = alloc::vec![0u32; (size * size) as usize];
        let mut at = 0usize;
        for pair in icon.runs.as_chunks::<2>().0 {
            let (count, index) = (usize::from(pair[0]), usize::from(pair[1]));
            let c: Color = if index == 0 {
                TRANSPARENT
            } else {
                let c = icon.palette.get(index).copied().unwrap_or(color::BLACK);
                if selected { darken(c) } else { c }
            };
            for p in row.iter_mut().skip(at).take(count) {
                *p = c;
            }
            at += count;
        }
        self.image_phys(x, y, size, size, &row, Some(TRANSPARENT));
    }

    /// Draws the 1-bit image of `icon` in `fg`.
    pub fn icon_mono(&mut self, x: i32, y: i32, icon: &Icon, fg: Color) {
        self.bitmap(x, y, icon.size as i32, icon.mono, fg, None);
    }
}

/// A value no icon colour has (they are 24-bit): the transparent pixel.
const TRANSPARENT: Color = 0xFF00_0000;

/// A colour at half brightness.
fn darken(c: Color) -> Color {
    (c >> 1) & 0x7F7F7F
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn every_icon_decodes_to_its_size() {
        assert!(!ALL.is_empty());
        for icon in ALL {
            let pairs = icon.runs.as_chunks::<2>().0;
            let pixels: u32 = pairs.iter().map(|p| u32::from(p[0])).sum();
            assert_eq!(pixels, icon.size * icon.size, "{}-{}", icon.name, icon.size);
            assert!(pairs.iter().all(|p| usize::from(p[1]) < icon.palette.len()));
            assert_eq!(icon.mono.len() as u32, icon.size.div_ceil(8) * icon.size);
        }
        for name in [
            "stela",
            "stela-cracked",
            "temple",
            "scroll",
            "kibotos",
            "volume",
            "app",
        ] {
            for size in [16, 32, 64, 128] {
                assert!(find(name, size).is_some(), "{name}-{size} missing");
            }
        }
    }

    #[test]
    fn drawing_keeps_transparency() {
        let icon = find("stela", 32).unwrap();
        let mut px = vec![7u32; 32 * 32];
        let mut c = Canvas::new(&mut px, 32, 32, 32);
        c.icon(0, 0, icon, false);
        assert_eq!(c.get(0, 0), Some(7), "the corner is transparent");
        assert!(c.pixels().contains(&color::TERRACOTTA));
        c.icon(0, 0, icon, true);
        assert!(!c.pixels().contains(&color::TERRACOTTA));
        assert_eq!(size_for(2560), 64);
    }
}
