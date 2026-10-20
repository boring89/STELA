// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Drawing on a pixel buffer (the toolbox's QuickDraw): rectangles,
//! lines, 1-bit and 32-bit bitmaps, and text in the Spleen fonts, all
//! clipped to the buffer and to a clip rectangle. Pixels are u32
//! `0x00RRGGBB`. Pure logic, tested on the host.

/// A pixel, `0x00RRGGBB`.
pub type Color = u32;

/// Colours used across STELA's interface (black-figure pottery: terracotta,
/// glaze black, parchment white; plus an Aegean grey-blue desktop).
pub mod color {
    use super::Color;
    /// Glaze black: lines and text.
    pub const BLACK: Color = 0x1A_1410;
    /// Parchment: menus, window backgrounds.
    pub const PARCHMENT: Color = 0xF2_E8D5;
    /// Terracotta: the active title bar.
    pub const TERRACOTTA: Color = 0xC8_763C;
    /// A pale stone colour: inactive title bars.
    pub const STONE: Color = 0xD8_CDB8;
    /// The desktop.
    pub const DESKTOP: Color = 0x5B_7B86;
    /// Highlight (selected menu item, selected icon).
    pub const HIGHLIGHT: Color = 0x2E_4A62;
    /// Pure white.
    pub const WHITE: Color = 0xFF_FFFF;
    /// Grey for disabled text.
    pub const GREY: Color = 0x8A_8070;
}

/// A rectangle: top-left corner and size (empty when either size is ≤ 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl Rect {
    /// A rectangle.
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    /// Whether it covers no pixel.
    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// One past the right edge.
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    /// One past the bottom edge.
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Whether the point is inside.
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The overlap (empty if none).
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        if r <= x || b <= y {
            Rect::default()
        } else {
            Rect::new(x, y, r - x, b - y)
        }
    }

    /// The smallest rectangle covering both (an empty one is ignored).
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Moved by (dx, dy).
    pub const fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// Shrunk by `n` on every side.
    pub const fn inset(&self, n: i32) -> Rect {
        Rect::new(self.x + n, self.y + n, self.w - 2 * n, self.h - 2 * n)
    }

    /// Whether the two overlap.
    pub fn overlaps(&self, other: &Rect) -> bool {
        !self.intersect(other).is_empty()
    }

    /// Pixels covered.
    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            i64::from(self.w) * i64::from(self.h)
        }
    }
}

/// A bitmap font: `height` rows of `width.div_ceil(8)` bytes per glyph,
/// MSB = leftmost pixel, for the code points `first..first + count`.
pub struct Font {
    /// Cell width.
    pub width: i32,
    /// Cell height.
    pub height: i32,
    first: u32,
    count: usize,
    data: &'static [u8],
}

impl Font {
    fn row_bytes(&self) -> usize {
        (self.width as usize).div_ceil(8)
    }

    /// The glyph rows of `c` (`?` for characters the font lacks).
    pub fn glyph(&self, c: char) -> &'static [u8] {
        let index = (c as u32)
            .checked_sub(self.first)
            .map(|i| i as usize)
            .filter(|&i| i < self.count)
            .unwrap_or(('?' as u32 - self.first) as usize);
        let size = self.height as usize * self.row_bytes();
        &self.data[index * size..(index + 1) * size]
    }

    /// Width of `text` in pixels.
    pub fn text_width(&self, text: &str) -> i32 {
        text.chars().count() as i32 * self.width
    }
}

mod spleen_8x16 {
    #![allow(missing_docs)]
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../third_party/spleen/spleen_8x16.rs"
    ));
}

mod spleen_16x32 {
    #![allow(missing_docs)]
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../third_party/spleen/spleen_16x32.rs"
    ));
}

/// Spleen 8x16 (BSD-2-Clause, `third_party/spleen`).
pub static SMALL: Font = Font {
    width: spleen_8x16::WIDTH as i32,
    height: spleen_8x16::HEIGHT as i32,
    first: spleen_8x16::FIRST,
    count: spleen_8x16::COUNT,
    data: &spleen_8x16::DATA,
};

/// Spleen 16x32.
pub static LARGE: Font = Font {
    width: spleen_16x32::WIDTH as i32,
    height: spleen_16x32::HEIGHT as i32,
    first: spleen_16x32::FIRST,
    count: spleen_16x32::COUNT,
    data: &spleen_16x32::DATA,
};

/// The interface font for a screen `width` pixels wide: 16x32 from 1920
/// up (`docs/display.md`), else 8x16.
pub fn font_for(width: u32) -> &'static Font {
    if width >= 1920 { &LARGE } else { &SMALL }
}

/// A pixel buffer to draw on: `height` rows of `stride` pixels, of which
/// `width` are visible. Everything is clipped to the buffer and to
/// [`Canvas::clip`].
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: i32,
    height: i32,
    stride: usize,
    clip: Rect,
}

impl<'a> Canvas<'a> {
    /// A canvas over `pixels` (at least `stride * height` of them).
    pub fn new(pixels: &'a mut [u32], width: u32, height: u32, stride: usize) -> Canvas<'a> {
        let height = height.min((pixels.len() / stride.max(1)) as u32);
        Canvas {
            pixels,
            width: width as i32,
            height: height as i32,
            stride,
            clip: Rect::new(0, 0, width as i32, height as i32),
        }
    }

    /// Visible size.
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// The whole canvas.
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Limits drawing to `clip` (within the canvas).
    pub fn set_clip(&mut self, clip: Rect) {
        self.clip = clip.intersect(&self.bounds());
    }

    /// The current clip rectangle.
    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Removes the clip (the whole canvas).
    pub fn reset_clip(&mut self) {
        self.clip = self.bounds();
    }

    /// The pixel at (x, y), if inside the canvas.
    pub fn get(&self, x: i32, y: i32) -> Option<Color> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        Some(self.pixels[y as usize * self.stride + x as usize])
    }

    /// Sets one pixel (if inside the clip).
    pub fn pixel(&mut self, x: i32, y: i32, color: Color) {
        if self.clip.contains(x, y) {
            self.pixels[y as usize * self.stride + x as usize] = color;
        }
    }

    /// Fills `rect`.
    pub fn fill(&mut self, rect: Rect, color: Color) {
        let r = rect.intersect(&self.clip);
        if r.is_empty() {
            return;
        }
        for y in r.y..r.bottom() {
            let row = y as usize * self.stride;
            self.pixels[row + r.x as usize..row + r.right() as usize].fill(color);
        }
    }

    /// A horizontal line of `len` pixels from (x, y).
    pub fn hline(&mut self, x: i32, y: i32, len: i32, color: Color) {
        self.fill(Rect::new(x, y, len, 1), color);
    }

    /// A vertical line of `len` pixels from (x, y).
    pub fn vline(&mut self, x: i32, y: i32, len: i32, color: Color) {
        self.fill(Rect::new(x, y, 1, len), color);
    }

    /// A one-pixel outline of `rect`.
    pub fn frame(&mut self, rect: Rect, color: Color) {
        if rect.is_empty() {
            return;
        }
        self.hline(rect.x, rect.y, rect.w, color);
        self.hline(rect.x, rect.bottom() - 1, rect.w, color);
        self.vline(rect.x, rect.y, rect.h, color);
        self.vline(rect.right() - 1, rect.y, rect.h, color);
    }

    /// A line from (x0, y0) to (x1, y1) (Bresenham).
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let (mut x, mut y) = (x0, y0);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.pixel(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// A 1-bit bitmap (`rows` of `w.div_ceil(8)` bytes, MSB first) at
    /// (x, y): set bits in `fg`, clear bits in `bg` (or left alone).
    pub fn bitmap(&mut self, x: i32, y: i32, w: i32, rows: &[u8], fg: Color, bg: Option<Color>) {
        let row_bytes = (w as usize).div_ceil(8);
        if row_bytes == 0 {
            return;
        }
        for (ry, row) in rows.chunks(row_bytes).enumerate() {
            for rx in 0..w {
                let on = row[rx as usize / 8] & (0x80 >> (rx % 8)) != 0;
                match (on, bg) {
                    (true, _) => self.pixel(x + rx, y + ry as i32, fg),
                    (false, Some(bg)) => self.pixel(x + rx, y + ry as i32, bg),
                    (false, None) => {}
                }
            }
        }
    }

    /// A 32-bit image of `w` x `h` pixels (rows of `w`) at (x, y); pixels
    /// equal to `transparent` are skipped.
    pub fn image(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        pixels: &[u32],
        transparent: Option<Color>,
    ) {
        let dest = Rect::new(x, y, w, h).intersect(&self.clip);
        if dest.is_empty() || pixels.len() < (w * h) as usize {
            return;
        }
        for py in dest.y..dest.bottom() {
            let src = ((py - y) * w) as usize;
            let row = py as usize * self.stride;
            for px in dest.x..dest.right() {
                let p = pixels[src + (px - x) as usize];
                if Some(p) != transparent {
                    self.pixels[row + px as usize] = p;
                }
            }
        }
    }

    /// Copies the `src` rectangle of another buffer (rows of `src_stride`
    /// pixels) to (x, y), clipped. Used to composite windows.
    pub fn copy_from(&mut self, x: i32, y: i32, src: &[u32], src_stride: usize, src_rect: Rect) {
        let dest = Rect::new(x, y, src_rect.w, src_rect.h).intersect(&self.clip);
        if dest.is_empty() {
            return;
        }
        for py in dest.y..dest.bottom() {
            let sy = (src_rect.y + py - y) as usize;
            let sx = (src_rect.x + dest.x - x) as usize;
            let from = sy * src_stride + sx;
            let Some(src_row) = src.get(from..from + dest.w as usize) else {
                return;
            };
            let row = py as usize * self.stride;
            self.pixels[row + dest.x as usize..row + dest.right() as usize]
                .copy_from_slice(src_row);
        }
    }

    /// One character cell at (x, y); with `bg`, the cell is filled first.
    pub fn char(&mut self, x: i32, y: i32, c: char, font: &Font, fg: Color, bg: Option<Color>) {
        self.bitmap(x, y, font.width, font.glyph(c), fg, bg);
    }

    /// Text from (x, y) (top-left of the first cell); returns the x after
    /// it.
    pub fn text(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        font: &Font,
        fg: Color,
        bg: Option<Color>,
    ) -> i32 {
        let mut at = x;
        for c in text.chars() {
            self.char(at, y, c, font, fg, bg);
            at += font.width;
        }
        at
    }

    /// The pixels (rows of [`Canvas::stride`]).
    pub fn pixels(&self) -> &[u32] {
        self.pixels
    }

    /// Pixels per row.
    pub fn stride(&self) -> usize {
        self.stride
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn rectangles() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Rect::new(5, 5, 5, 5));
        assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
        assert!(a.intersect(&Rect::new(10, 0, 5, 5)).is_empty());
        assert_eq!(Rect::default().union(&b), b);
        assert!(a.contains(9, 9) && !a.contains(10, 9));
        assert_eq!(a.inset(2), Rect::new(2, 2, 6, 6));
        assert_eq!(a.offset(3, -1), Rect::new(3, -1, 10, 10));
        assert_eq!(b.area(), 100);
        assert_eq!(Rect::new(0, 0, -1, 5).area(), 0);
    }

    #[test]
    fn fill_and_clip() {
        let mut px = vec![0u32; 8 * 4];
        {
            let mut c = Canvas::new(&mut px, 6, 4, 8);
            c.fill(Rect::new(-2, -2, 4, 4), 1);
            assert_eq!(c.get(0, 0), Some(1));
            assert_eq!(c.get(1, 1), Some(1));
            assert_eq!(c.get(2, 2), Some(0));
            c.set_clip(Rect::new(3, 0, 2, 4));
            c.fill(Rect::new(0, 0, 100, 100), 2);
            assert_eq!(c.get(2, 0), Some(0));
            assert_eq!(c.get(4, 3), Some(2));
            assert_eq!(c.get(5, 3), Some(0));
        }
        // The padding past `width` is never touched.
        assert_eq!(px[6], 0);
        assert_eq!(px[7], 0);
    }

    #[test]
    fn lines_frames_and_bitmaps() {
        let mut px = vec![0u32; 10 * 10];
        let mut c = Canvas::new(&mut px, 10, 10, 10);
        c.frame(Rect::new(1, 1, 5, 4), 3);
        assert_eq!(c.get(1, 1), Some(3));
        assert_eq!(c.get(5, 4), Some(3));
        assert_eq!(c.get(3, 2), Some(0));
        c.line(0, 9, 9, 0, 4);
        assert_eq!(c.get(0, 9), Some(4));
        assert_eq!(c.get(9, 0), Some(4));
        assert_eq!(c.get(5, 4), Some(4));
        c.bitmap(0, 0, 3, &[0b1010_0000, 0b0100_0000], 7, Some(8));
        assert_eq!(
            (c.get(0, 0), c.get(1, 0), c.get(2, 0)),
            (Some(7), Some(8), Some(7))
        );
        assert_eq!(c.get(1, 1), Some(7));
    }

    #[test]
    fn images_copies_and_text() {
        let mut px = vec![0u32; 6 * 6];
        let mut c = Canvas::new(&mut px, 6, 6, 6);
        c.image(4, 4, 3, 3, &[1, 2, 3, 4, 9, 6, 7, 8, 9], Some(9));
        assert_eq!(c.get(4, 4), Some(1));
        assert_eq!(c.get(5, 5), Some(0)); // transparent
        assert_eq!(c.get(5, 4), Some(2));
        let src = [10, 11, 12, 13, 14, 15, 16, 17, 18];
        c.copy_from(0, 0, &src, 3, Rect::new(1, 1, 2, 2));
        assert_eq!((c.get(0, 0), c.get(1, 1)), (Some(14), Some(18)));
        let mut px = vec![0u32; 40 * 16];
        let mut c = Canvas::new(&mut px, 40, 16, 40);
        let end = c.text(0, 0, "Hi", &SMALL, 5, None);
        assert_eq!(end, 16);
        assert!(c.pixels().contains(&5));
        assert_eq!(SMALL.text_width("abc"), 24);
        assert_eq!(font_for(2560).width, 16);
        assert_eq!(font_for(1280).width, 8);
        assert_eq!(SMALL.glyph('\u{1F600}'), SMALL.glyph('?'));
    }
}
