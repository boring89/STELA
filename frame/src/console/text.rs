// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A text console on a pixel [`Canvas`]: a grid of character cells with a
//! cursor, line wrapping and scrolling. Pure logic, tested on the host with
//! an in-memory canvas; `fb.rs` supplies the real framebuffer.

use super::font::Font;

/// A colour as 8-bit red, green, blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Something that can be drawn on, `width` x `height` pixels.
pub trait Canvas {
    /// `(width, height)` in pixels.
    fn size(&self) -> (usize, usize);
    /// Sets one pixel; coordinates outside the canvas are ignored.
    fn set_pixel(&mut self, x: usize, y: usize, color: Rgb);
    /// Fills a rectangle (clipped to the canvas).
    fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, color: Rgb) {
        let (w, h) = self.size();
        for py in y..(y + height).min(h) {
            for px in x..(x + width).min(w) {
                self.set_pixel(px, py, color);
            }
        }
    }
    /// Moves everything up by `rows` pixel rows and fills the freed rows at
    /// the bottom with `color`.
    fn scroll_up(&mut self, rows: usize, color: Rgb);
}

/// Draws `c` with its top-left corner at pixel (`x`, `y`).
pub fn draw_char(
    canvas: &mut impl Canvas,
    font: &Font,
    x: usize,
    y: usize,
    c: char,
    fg: Rgb,
    bg: Rgb,
) {
    let glyph = font.glyph(c);
    for gy in 0..font.height {
        for gx in 0..font.width {
            let color = if glyph.pixel(gx, gy) { fg } else { bg };
            canvas.set_pixel(x + gx, y + gy, color);
        }
    }
}

/// Cursor state of a text console.
#[derive(Debug)]
pub struct TextConsole {
    font: &'static Font,
    columns: usize,
    rows: usize,
    column: usize,
    row: usize,
    /// Text colour.
    pub fg: Rgb,
    /// Background colour.
    pub bg: Rgb,
}

/// Tab stops every 8 columns.
const TAB: usize = 8;

impl TextConsole {
    /// A console filling a `width` x `height` canvas with `font`.
    pub fn new(font: &'static Font, width: usize, height: usize, fg: Rgb, bg: Rgb) -> TextConsole {
        TextConsole {
            font,
            columns: (width / font.width).max(1),
            rows: (height / font.height).max(1),
            column: 0,
            row: 0,
            fg,
            bg,
        }
    }

    /// `(columns, rows)` of the grid.
    pub const fn grid(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }

    /// `(column, row)` of the cursor.
    pub const fn cursor(&self) -> (usize, usize) {
        (self.column, self.row)
    }

    /// Clears the canvas and homes the cursor.
    pub fn clear(&mut self, canvas: &mut impl Canvas) {
        let (w, h) = canvas.size();
        canvas.fill(0, 0, w, h, self.bg);
        self.column = 0;
        self.row = 0;
    }

    fn newline(&mut self, canvas: &mut impl Canvas) {
        self.column = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            canvas.scroll_up(self.font.height, self.bg);
        }
    }

    /// Writes one character: `\n` moves to the next line, `\r` to the
    /// start of the line, `\t` to the next tab stop; anything else is drawn
    /// (unknown characters as `?`), wrapping at the right edge.
    pub fn write_char(&mut self, canvas: &mut impl Canvas, c: char) {
        match c {
            '\n' => self.newline(canvas),
            '\r' => self.column = 0,
            '\t' => {
                let next = (self.column / TAB + 1) * TAB;
                while self.column < next.min(self.columns) {
                    self.write_char(canvas, ' ');
                    if self.column == 0 {
                        break;
                    }
                }
            }
            c => {
                if self.column >= self.columns {
                    self.newline(canvas);
                }
                draw_char(
                    canvas,
                    self.font,
                    self.column * self.font.width,
                    self.row * self.font.height,
                    c,
                    self.fg,
                    self.bg,
                );
                self.column += 1;
            }
        }
    }

    /// Writes a string.
    pub fn write_str(&mut self, canvas: &mut impl Canvas, s: &str) {
        for c in s.chars() {
            self.write_char(canvas, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::font::SPLEEN_8X16;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    const FG: Rgb = Rgb(255, 255, 255);
    const BG: Rgb = Rgb(0, 0, 0);

    struct Memory {
        width: usize,
        height: usize,
        pixels: Vec<Rgb>,
    }

    impl Memory {
        fn new(width: usize, height: usize) -> Memory {
            Memory {
                width,
                height,
                pixels: vec![Rgb(1, 2, 3); width * height],
            }
        }

        /// Whether the 8x16 cell at (column, row) has any foreground pixel.
        fn cell_inked(&self, column: usize, row: usize) -> bool {
            (0..16).any(|y| {
                (0..8).any(|x| self.pixels[(row * 16 + y) * self.width + column * 8 + x] == FG)
            })
        }
    }

    impl Canvas for Memory {
        fn size(&self) -> (usize, usize) {
            (self.width, self.height)
        }
        fn set_pixel(&mut self, x: usize, y: usize, color: Rgb) {
            if x < self.width && y < self.height {
                self.pixels[y * self.width + x] = color;
            }
        }
        fn scroll_up(&mut self, rows: usize, color: Rgb) {
            let shift = rows * self.width;
            self.pixels.copy_within(shift.., 0);
            let len = self.pixels.len();
            self.pixels[len - shift..].fill(color);
        }
    }

    fn console(memory: &mut Memory) -> TextConsole {
        let mut console = TextConsole::new(&SPLEEN_8X16, memory.width, memory.height, FG, BG);
        console.clear(memory);
        console
    }

    #[test]
    fn grid_from_canvas_size() {
        let mut memory = Memory::new(1280, 800);
        let console = console(&mut memory);
        assert_eq!(console.grid(), (160, 50));
        assert!(memory.pixels.iter().all(|&p| p == BG));
    }

    #[test]
    fn writes_wraps_and_breaks_lines() {
        let mut memory = Memory::new(32, 48); // 4 x 3 cells
        let mut console = console(&mut memory);
        console.write_str(&mut memory, "AB C");
        assert_eq!(console.cursor(), (4, 0));
        assert!(memory.cell_inked(0, 0) && memory.cell_inked(1, 0));
        assert!(!memory.cell_inked(2, 0));
        console.write_char(&mut memory, 'D'); // wraps
        assert_eq!(console.cursor(), (1, 1));
        assert!(memory.cell_inked(0, 1));
        console.write_str(&mut memory, "\nE\rF");
        assert_eq!(console.cursor(), (1, 2));
        assert!(memory.cell_inked(0, 2));
    }

    #[test]
    fn scrolls_at_the_bottom() {
        let mut memory = Memory::new(32, 48);
        let mut console = console(&mut memory);
        console.write_str(&mut memory, "A\n\nB\nC");
        // "A" scrolled off the top; "B" moved from row 2 to row 1.
        assert_eq!(console.cursor(), (1, 2));
        assert!(!memory.cell_inked(0, 0));
        assert!(memory.cell_inked(0, 1));
        assert!(memory.cell_inked(0, 2));
    }

    #[test]
    fn tabs_stop_every_eight_columns() {
        let mut memory = Memory::new(160, 32); // 20 x 2 cells
        let mut console = console(&mut memory);
        console.write_str(&mut memory, "ab\tc");
        assert_eq!(console.cursor(), (9, 0));
        console.write_str(&mut memory, "\t\t");
        assert_eq!(console.cursor(), (20, 0));
    }
}
