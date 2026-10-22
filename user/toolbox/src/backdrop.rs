// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's patterns (P13 K20, drafts like the icons): the window
//! server draws them behind the windows and Mouseion in its icon strip, on
//! the same screen grid, so that they meet. `Data:/.stela/desktop` holds
//! `pattern=plain|meander|waves` and `color=RRGGBB`
//! (`abi::window::backdrop`).

use crate::draw::{Canvas, Rect};
use abi::window::backdrop;

/// The Greek key: a 16x16 tile, `X` on the line.
const MEANDER_TILE: [&[u8; 16]; 16] = [
    b"XXXXXXXXXXXXXXX.",
    b"X.............X.",
    b"X.XXXXXXXXXXX.X.",
    b"X.X.........X.X.",
    b"X.X.XXXXXXX.X.X.",
    b"X.X.X.....X.X.X.",
    b"X.X.X.XXX.X.X.X.",
    b"X.X.X.X.X.X.X.X.",
    b"X.X.X.X...X.X.X.",
    b"X.X.X.XXXXX.X.X.",
    b"X.X.X.......X.X.",
    b"X.X.XXXXXXXXX.X.",
    b"X.X...........X.",
    b"X.XXXXXXXXXXXXX.",
    b"X...............",
    b"XXXXXXXXXXXXXXXX",
];

/// The waves' height at each column of their 16-pixel period.
const WAVE: [i32; 16] = [0, 1, 2, 3, 3, 3, 2, 1, 0, -1, -2, -3, -3, -3, -2, -1];

/// Whether point (x, y) of the screen (logical pixels) is on `pattern`'s
/// line.
pub fn on(pattern: u32, x: i32, y: i32) -> bool {
    let (tx, ty) = (x.rem_euclid(16) as usize, y.rem_euclid(16));
    match pattern {
        backdrop::MEANDER => MEANDER_TILE[ty as usize][tx] == b'X',
        backdrop::WAVES => {
            // A line from this column's height to the next one's.
            let a = 8 + WAVE[tx];
            let b = 8 + WAVE[(tx + 1) % 16];
            (a.min(b)..=a.max(b)).contains(&ty)
        }
        _ => false,
    }
}

/// The pattern's line colour on `base`: a little darker.
pub fn line_color(base: u32) -> u32 {
    let ch = |shift: u32| ((base >> shift) & 0xFF) * 13 / 16;
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// `Data:/.stela/desktop`: the pattern and the colour (`None`: the usual
/// one).
pub fn parse(text: &str) -> (u32, Option<u32>) {
    let mut pattern = backdrop::PLAIN;
    let mut color = None;
    for line in text.lines() {
        match line.trim().split_once('=') {
            Some(("pattern", v)) => pattern = backdrop::from_name(v.trim()),
            Some(("color", v)) => {
                color = u32::from_str_radix(v.trim().trim_start_matches('#'), 16)
                    .ok()
                    .filter(|c| *c <= 0xFF_FFFF);
            }
            _ => {}
        }
    }
    (pattern, color)
}

/// Paints `r` of `c` (whose top left is at `origin` on the screen) with
/// the desktop: `base`, and `pattern`'s line.
pub fn paint(c: &mut Canvas, r: Rect, origin: (i32, i32), pattern: u32, base: u32) {
    c.fill(r, base);
    if pattern == backdrop::PLAIN {
        return;
    }
    let line = line_color(base);
    for y in r.y..r.bottom() {
        let mut x = r.x;
        while x < r.right() {
            // Runs of line pixels as one fill.
            if on(pattern, origin.0 + x, origin.1 + y) {
                let start = x;
                while x < r.right() && on(pattern, origin.0 + x, origin.1 + y) {
                    x += 1;
                }
                c.fill(Rect::new(start, y, x - start, 1), line);
            } else {
                x += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_patterns() {
        assert!(!on(backdrop::PLAIN, 0, 0));
        assert!(on(backdrop::MEANDER, 0, 0));
        assert!(!on(backdrop::MEANDER, 1, 1));
        assert!(on(backdrop::MEANDER, 16, 0), "it repeats");
        assert!(on(backdrop::MEANDER, -16, -1), "and to the left and up");
        // The waves: a line in every column.
        for x in 0..16 {
            assert!((0..16).any(|y| on(backdrop::WAVES, x, y)));
        }
        assert!(on(backdrop::WAVES, 0, 8));
        assert_eq!(line_color(0x10_2030), 0x0D_1A27);
    }

    #[test]
    fn the_settings_file() {
        assert_eq!(
            parse("pattern=waves\ncolor=#102030\n"),
            (backdrop::WAVES, Some(0x10_2030))
        );
        assert_eq!(parse("pattern=what\ncolor=zz"), (backdrop::PLAIN, None));
    }

    #[test]
    fn painting() {
        let mut px = alloc::vec![0u32; 32 * 4];
        let mut c = Canvas::new(&mut px, 32, 4, 32);
        paint(
            &mut c,
            Rect::new(0, 0, 32, 4),
            (16, 0),
            backdrop::MEANDER,
            0x5B_7B86,
        );
        let line = line_color(0x5B_7B86);
        // The screen's column 16 is the tile's first: the line.
        assert_eq!((px[0], px[1], px[32 + 1]), (line, line, 0x5B_7B86));
        assert_eq!(px[15], 0x5B_7B86);
    }
}
