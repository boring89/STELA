// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Characters beyond ASCII (P11 K39, `docs/toolbox.md` §6.3): a 12-pixel
//! font with Chinese, Japanese, Korean and the rest of the BMP (Fusion
//! Pixel 12px, SIL OFL 1.1, `third_party/fusion-pixel/`), read from
//! `Initrd:/fonts/` the first time a program draws such a character.
//!
//! ```text
//!  file: "STF1" count 12 28 | records sorted by code point:
//!        u16 code point, u8 advance (6 or 12 px), u8 0, 12 x u16 rows
//!  a 12 px wide glyph takes two cells (a CJK character), a 6 px one one
//!  cell; drawn 1x in a 16-pixel row (8x16 font), 2x in a 32-pixel row
//! ```
//!
//! The lookup is pure and tested on the host; loading needs the file
//! system.

use alloc::sync::Arc;
use alloc::vec::Vec;

/// Rows a glyph has.
pub const ROWS: usize = 12;
const RECORD: usize = 4 + 2 * ROWS;
const HEADER: usize = 12;

/// Where the font file is.
pub const PATH: &str = "fonts/fusion-pixel-12-zh_tw.bin";

/// The glyph file.
pub struct CjkFont {
    data: Vec<u8>,
    count: usize,
}

/// A glyph: its advance in pixels (6 or 12) and its rows (bit 15 is the
/// leftmost pixel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    /// 6 (one cell) or 12 (two cells).
    pub advance: u8,
    /// The rows, top first.
    pub rows: [u16; ROWS],
}

impl CjkFont {
    /// The font in `data`, if it is one.
    pub fn parse(data: Vec<u8>) -> Option<CjkFont> {
        if data.len() < HEADER || &data[..4] != b"STF1" {
            return None;
        }
        let count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let rows = u16::from_le_bytes([data[8], data[9]]) as usize;
        let record = u16::from_le_bytes([data[10], data[11]]) as usize;
        if rows != ROWS || record != RECORD || data.len() < HEADER + count * RECORD {
            return None;
        }
        Some(CjkFont { data, count })
    }

    fn code(&self, i: usize) -> u32 {
        let at = HEADER + i * RECORD;
        u32::from(u16::from_le_bytes([self.data[at], self.data[at + 1]]))
    }

    /// The glyph of `c`, if the font has it.
    pub fn glyph(&self, c: char) -> Option<Glyph> {
        let c = c as u32;
        let (mut lo, mut hi) = (0, self.count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.code(mid).cmp(&c) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => {
                    let at = HEADER + mid * RECORD;
                    let mut rows = [0u16; ROWS];
                    for (r, row) in rows.iter_mut().enumerate() {
                        let b = at + 4 + 2 * r;
                        *row = u16::from_le_bytes([self.data[b], self.data[b + 1]]);
                    }
                    return Some(Glyph {
                        advance: self.data[at + 2],
                        rows,
                    });
                }
            }
        }
        None
    }

    /// Glyphs in the font.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// The loaded font: not tried yet, tried and missing, or there.
static FONT: libstela::sync::SpinMutex<Option<Option<Arc<CjkFont>>>> =
    libstela::sync::SpinMutex::new(None);

/// Uses `font` from now on (tests; or a program that loaded it itself).
pub fn set_font(font: Option<CjkFont>) {
    *FONT.lock() = Some(font.map(Arc::new));
}

/// The font, loaded the first time (`None` if there is no file).
pub fn font() -> Option<Arc<CjkFont>> {
    let mut f = FONT.lock();
    if f.is_none() {
        *f = Some(load().map(Arc::new));
    }
    f.as_ref().and_then(Clone::clone)
}

#[cfg(not(test))]
fn load() -> Option<CjkFont> {
    // P14 K4: on the System volume (the initrd has no CJK font).
    let data = libstela::fs::read_system(PATH).ok()?;
    CjkFont::parse(data)
}

#[cfg(test)]
fn load() -> Option<CjkFont> {
    None
}

/// Cells `c` takes: 2 for a character the font draws 12 pixels wide (CJK),
/// else 1. ASCII never asks for the font.
pub fn cells(c: char) -> usize {
    if (c as u32) < 0x80 {
        return 1;
    }
    match font().and_then(|f| f.glyph(c)) {
        Some(g) if g.advance > 6 => 2,
        _ => 1,
    }
}

/// Cells `text` takes.
pub fn width(text: &str) -> usize {
    text.chars().map(cells).sum()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small font: 'é' one cell, '中' two cells.
    pub(crate) fn sample() -> CjkFont {
        let mut d = Vec::new();
        d.extend_from_slice(b"STF1");
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&12u16.to_le_bytes());
        d.extend_from_slice(&28u16.to_le_bytes());
        for (cp, adv, row) in [(0xE9u16, 6u8, 0xF000u16), (0x4E2D, 12, 0xFFF0)] {
            d.extend_from_slice(&cp.to_le_bytes());
            d.push(adv);
            d.push(0);
            for _ in 0..ROWS {
                d.extend_from_slice(&row.to_le_bytes());
            }
        }
        CjkFont::parse(d).expect("a font")
    }

    #[test]
    fn lookup() {
        let f = sample();
        assert_eq!(f.len(), 2);
        assert_eq!(f.glyph('中').map(|g| g.advance), Some(12));
        assert_eq!(f.glyph('é').map(|g| g.rows[0]), Some(0xF000));
        assert!(f.glyph('文').is_none());
        assert!(CjkFont::parse(alloc::vec![0; 8]).is_none());
    }

    #[test]
    fn the_real_font() {
        let data = include_bytes!("../../../third_party/fusion-pixel/fusion-pixel-12-zh_tw.bin");
        let f = CjkFont::parse(data.to_vec()).expect("the font file");
        assert!(f.len() > 30_000);
        // 你 (U+4F60): two cells; its second row has strokes.
        let g = f.glyph('你').expect("你");
        assert_eq!(g.advance, 12);
        assert_eq!(g.rows[1], 0b0010_1000_0000_0000);
        assert!(f.glyph('\u{378}').is_none());
    }
}
