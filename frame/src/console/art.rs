// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! STELA's mark for the screens the kernel draws itself
//! (`docs/identity.md`): the stela on the boot screen and the cracked
//! stela on the panic screen, from `assets/` through `tools/img_to_rs.py`
//! (the same data the toolbox uses). **The images are drafts.**

use super::text::{Canvas, Rgb};

/// An icon as `tools/img_to_rs.py` writes it.
pub struct Icon {
    /// Its name.
    pub name: &'static str,
    /// Width and height.
    pub size: u32,
    /// Colours `0x00RRGGBB`; index 0 is transparent.
    pub palette: &'static [u32],
    /// (count, palette index) pairs, row by row.
    pub runs: &'static [u8],
    /// 1-bit rows (not used by the kernel).
    pub mono: &'static [u8],
}

/// The logo (128x128, DRAFT).
pub static STELA: Icon = include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../assets/generated/stela-128.rs"
));

/// The cracked logo for the panic screen (128x128, DRAFT).
pub static STELA_CRACKED: Icon = include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../assets/generated/stela-cracked-128.rs"
));

/// Draws `icon` at (x, y), each pixel `scale` x `scale`, without
/// allocating (the panic screen uses it).
pub fn draw(canvas: &mut impl Canvas, x: usize, y: usize, icon: &Icon, scale: usize) {
    let size = icon.size as usize;
    let mut at = 0usize;
    for pair in icon.runs.as_chunks::<2>().0 {
        let (count, index) = (usize::from(pair[0]), usize::from(pair[1]));
        if index != 0
            && let Some(&c) = icon.palette.get(index)
        {
            let rgb = Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
            for k in at..at + count {
                let (px, py) = (k % size, k / size);
                canvas.fill(x + px * scale, y + py * scale, scale, scale, rgb);
            }
        }
        at += count;
    }
}

/// The scale the screens use: 2 from 1920 pixels wide, else 1.
pub fn scale_for(width: usize) -> usize {
    if width >= 1920 { 2 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    struct Grid {
        w: usize,
        px: Vec<Option<Rgb>>,
    }

    impl Canvas for Grid {
        fn size(&self) -> (usize, usize) {
            (self.w, self.px.len() / self.w)
        }
        fn set_pixel(&mut self, x: usize, y: usize, c: Rgb) {
            if x < self.w && y < self.px.len() / self.w {
                self.px[y * self.w + x] = Some(c);
            }
        }
        fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, c: Rgb) {
            for yy in y..y + h {
                for xx in x..x + w {
                    self.set_pixel(xx, yy, c);
                }
            }
        }
        fn scroll_up(&mut self, _: usize, _: Rgb) {}
    }

    #[test]
    fn the_stelae_draw() {
        for icon in [&STELA, &STELA_CRACKED] {
            assert_eq!(icon.size, 128);
            let mut g = Grid {
                w: 256,
                px: vec![None; 256 * 256],
            };
            draw(&mut g, 0, 0, icon, 2);
            assert_eq!(g.px[0], None, "the corner is transparent");
            assert!(g.px.contains(&Some(Rgb(0xC8, 0x76, 0x3C))));
        }
        // The crack: the middle of the cracked stela's face is clear.
        let mut g = Grid {
            w: 128,
            px: vec![None; 128 * 128],
        };
        draw(&mut g, 0, 0, &STELA_CRACKED, 1);
        let middle = 64 * 128 + 64;
        assert!((middle - 6..middle + 6).any(|i| g.px[i].is_none()));
        assert_eq!(scale_for(2560), 2);
    }
}
