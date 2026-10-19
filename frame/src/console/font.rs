// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bitmap fonts for the framebuffer console: Spleen 8x16 and 16x32
//! (BSD-2-Clause, `third_party/spleen/`), printable ASCII only.

/// A fixed-size bitmap font.
#[derive(Debug)]
pub struct Font {
    /// Cell width in pixels.
    pub width: usize,
    /// Cell height in pixels.
    pub height: usize,
    first: u32,
    count: usize,
    /// Glyphs: `height` rows of `width.div_ceil(8)` bytes, MSB = leftmost.
    data: &'static [u8],
}

impl Font {
    /// Bytes per glyph row.
    const fn row_bytes(&self) -> usize {
        self.width.div_ceil(8)
    }

    /// The glyph for `c`, or the one for `?` if the font lacks it.
    pub fn glyph(&self, c: char) -> Glyph<'_> {
        let index = (c as u32)
            .checked_sub(self.first)
            .map(|i| i as usize)
            .filter(|&i| i < self.count)
            .unwrap_or(('?' as u32 - self.first) as usize);
        let size = self.height * self.row_bytes();
        Glyph {
            font: self,
            bytes: &self.data[index * size..(index + 1) * size],
        }
    }
}

/// One glyph's bitmap.
#[derive(Clone, Copy, Debug)]
pub struct Glyph<'a> {
    font: &'a Font,
    bytes: &'a [u8],
}

impl Glyph<'_> {
    /// Whether pixel (`x`, `y`) of the cell is set.
    pub fn pixel(&self, x: usize, y: usize) -> bool {
        let row = self.font.row_bytes();
        x < self.font.width
            && y < self.font.height
            && self.bytes[y * row + x / 8] & (0x80 >> (x % 8)) != 0
    }
}

mod spleen_8x16 {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../third_party/spleen/spleen_8x16.rs"
    ));
}

mod spleen_16x32 {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../third_party/spleen/spleen_16x32.rs"
    ));
}

/// Spleen 8x16, for screens narrower than 1920 pixels.
pub static SPLEEN_8X16: Font = Font {
    width: spleen_8x16::WIDTH,
    height: spleen_8x16::HEIGHT,
    first: spleen_8x16::FIRST,
    count: spleen_8x16::COUNT,
    data: &spleen_8x16::DATA,
};

/// Spleen 16x32, for screens 1920 pixels wide or more.
pub static SPLEEN_16X32: Font = Font {
    width: spleen_16x32::WIDTH,
    height: spleen_16x32::HEIGHT,
    first: spleen_16x32::FIRST,
    count: spleen_16x32::COUNT,
    data: &spleen_16x32::DATA,
};

/// The font for a screen `width` pixels wide.
pub fn for_width(width: usize) -> &'static Font {
    if width >= 1920 {
        &SPLEEN_16X32
    } else {
        &SPLEEN_8X16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(font: &Font, c: char) -> std::vec::Vec<std::string::String> {
        let glyph = font.glyph(c);
        (0..font.height)
            .map(|y| {
                (0..font.width)
                    .map(|x| if glyph.pixel(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn sizes() {
        assert_eq!((SPLEEN_8X16.width, SPLEEN_8X16.height), (8, 16));
        assert_eq!((SPLEEN_16X32.width, SPLEEN_16X32.height), (16, 32));
        assert_eq!(SPLEEN_8X16.data.len(), 95 * 16);
        assert_eq!(SPLEEN_16X32.data.len(), 95 * 32 * 2);
        assert_eq!(for_width(1280).width, 8);
        assert_eq!(for_width(1920).width, 16);
    }

    #[test]
    fn letter_a_8x16() {
        let rows = render(&SPLEEN_8X16, 'A');
        assert_eq!(rows[2], ".#####..");
        assert_eq!(rows[6], "#######.");
        assert_eq!(rows[11], "##...##.");
        assert_eq!(rows[0], "........");
    }

    #[test]
    fn space_is_empty_and_unknown_is_question_mark() {
        for font in [&SPLEEN_8X16, &SPLEEN_16X32] {
            assert!(render(font, ' ').iter().all(|r| !r.contains('#')));
            assert_eq!(render(font, 'é'), render(font, '?'));
            assert_eq!(render(font, '\u{7}'), render(font, '?'));
            assert!(render(font, '~').iter().any(|r| r.contains('#')));
        }
    }

    #[test]
    fn wide_font_uses_both_bytes() {
        let rows = render(&SPLEEN_16X32, 'M');
        assert!(
            rows.iter()
                .any(|r| r[..8].contains('#') && r[8..].contains('#'))
        );
    }
}
