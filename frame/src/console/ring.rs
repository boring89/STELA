// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The last lines the kernel printed (P11, `docs/p520.md` §3.1): a fixed
//! ring that never allocates, so that the panic screen can show what
//! happened just before the panic.
//!
//! ```text
//!  kprint! ──► write(s) ──► partial line ──'\n'──► lines[next] (oldest
//!                                                  overwritten)
//!  panic screen ──► last(10): oldest first
//! ```

/// Lines kept.
pub const LINES: usize = 16;
/// Bytes kept of each line (the rest is cut).
pub const WIDTH: usize = 120;

/// The ring.
pub struct LineRing {
    lines: [[u8; WIDTH]; LINES],
    lens: [usize; LINES],
    /// Where the next finished line goes.
    next: usize,
    /// Finished lines in the ring (at most `LINES`).
    count: usize,
    partial: [u8; WIDTH],
    partial_len: usize,
}

impl LineRing {
    /// An empty ring.
    pub const fn new() -> LineRing {
        LineRing {
            lines: [[0; WIDTH]; LINES],
            lens: [0; LINES],
            next: 0,
            count: 0,
            partial: [0; WIDTH],
            partial_len: 0,
        }
    }

    /// Adds printed text; every `\n` finishes a line.
    pub fn write(&mut self, s: &str) {
        for &b in s.as_bytes() {
            match b {
                b'\n' => self.finish(),
                b'\r' => {}
                _ if self.partial_len < WIDTH => {
                    self.partial[self.partial_len] = b;
                    self.partial_len += 1;
                }
                _ => {}
            }
        }
    }

    fn finish(&mut self) {
        self.lines[self.next] = self.partial;
        self.lens[self.next] = self.partial_len;
        self.next = (self.next + 1) % LINES;
        self.count = (self.count + 1).min(LINES);
        self.partial_len = 0;
    }

    /// The last `n` finished lines (fewer if there are not as many),
    /// oldest first; a line cut inside a character ends before it.
    pub fn last(&self, n: usize) -> impl Iterator<Item = &str> {
        let n = n.min(self.count);
        (0..n).map(move |i| {
            let at = (self.next + LINES - n + i) % LINES;
            let bytes = &self.lines[at][..self.lens[at]];
            match core::str::from_utf8(bytes) {
                Ok(s) => s,
                Err(e) => core::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or(""),
            }
        })
    }
}

impl Default for LineRing {
    fn default() -> LineRing {
        LineRing::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec::Vec;

    #[test]
    fn keeps_the_last_lines() {
        let mut r = LineRing::new();
        assert_eq!(r.last(10).count(), 0);
        r.write("one\ntwo\nthr");
        // The unfinished line is not shown.
        assert_eq!(r.last(10).collect::<Vec<_>>(), ["one", "two"]);
        r.write("ee\r\n");
        assert_eq!(r.last(2).collect::<Vec<_>>(), ["two", "three"]);
        for i in 0..20 {
            r.write(&format!("line {i}\n"));
        }
        let last: Vec<_> = r.last(10).collect();
        assert_eq!(last.len(), 10);
        assert_eq!(last[0], "line 10");
        assert_eq!(last[9], "line 19");
        assert_eq!(r.last(100).count(), LINES);
    }

    #[test]
    fn long_lines_are_cut() {
        let mut r = LineRing::new();
        let long = "x".repeat(WIDTH + 30);
        r.write(&long);
        r.write("\n");
        assert_eq!(r.last(1).next().map(str::len), Some(WIDTH));
        // A character across the cut is left out.
        let s = format!("{}é\n", "y".repeat(WIDTH - 1));
        r.write(&s);
        assert_eq!(r.last(1).next().map(str::len), Some(WIDTH - 1));
    }
}
