// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The bookkeeping of the audio thread (`docs/sound.md`), as pure logic:
//! where the controller is in the cyclic DMA buffer (counted as an
//! ever-growing byte position) and how far ahead of it the buffer is
//! filled.
//!
//! ```text
//!  cyclic buffer:  ...played...|hw ──── lead ────| fill |...stale...
//!  every tick: hw advances by (LPIB - last LPIB) mod size; the buffer is
//!  filled from `fill` up to hw + lead: from the program's ring while it
//!  has bytes, with silence when it runs dry (an underrun never replays
//!  old samples)
//! ```

/// Bytes kept filled ahead of the controller (about 85 ms at 48 kHz,
/// 16-bit stereo).
pub const LEAD: u64 = 16 * 1024;

/// Positions of one playback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Feeder {
    /// Bytes the controller has read, ever.
    pub hw: u64,
    /// Bytes written into the DMA buffer, ever.
    pub fill: u64,
    last_lpib: usize,
    size: usize,
}

impl Feeder {
    /// A playback over a `size`-byte cyclic buffer whose controller
    /// position is `lpib`; filling starts at that position.
    pub fn new(size: usize, lpib: usize) -> Feeder {
        Feeder {
            hw: lpib as u64,
            fill: lpib as u64,
            last_lpib: lpib,
            size,
        }
    }

    /// The controller is at `lpib` now; returns how many bytes to write
    /// at `fill` (in `size`-bounded pieces, see [`Feeder::piece`]). If the
    /// controller overtook the filled part (the thread was late), filling
    /// jumps to its position.
    pub fn advance(&mut self, lpib: usize) -> u64 {
        let delta = (lpib + self.size - self.last_lpib) % self.size;
        self.last_lpib = lpib;
        self.hw += delta as u64;
        if self.fill < self.hw {
            self.fill = self.hw;
        }
        (self.hw + LEAD).saturating_sub(self.fill)
    }

    /// The next piece to write: (offset in the buffer, bytes), at most
    /// `want`, never across the buffer's end.
    pub fn piece(&self, want: u64) -> (usize, usize) {
        let at = (self.fill % self.size as u64) as usize;
        (at, (want as usize).min(self.size - at))
    }

    /// `n` bytes were written.
    pub fn wrote(&mut self, n: usize) {
        self.fill += n as u64;
    }
}

/// From a ring whose positions are `write` and `read` (u32, wrapping):
/// how many of the `want` bytes come from the ring.
pub fn from_ring(write: u32, read: u32, want: usize) -> usize {
    (write.wrapping_sub(read) as usize).min(want)
}

/// Adds `bytes` (16-bit little-endian samples) at `volume` (0-100) into
/// `acc` (P13 K18: mixing several programs).
pub fn mix_into(acc: &mut [i32], bytes: &[u8], volume: u8) {
    let v = i32::from(volume.min(100));
    for (a, s) in acc.iter_mut().zip(bytes.as_chunks::<2>().0) {
        let sample = i32::from(i16::from_le_bytes(*s));
        *a += sample * v / 100;
    }
}

/// The mixed samples at the master `volume` (0-100; nothing when
/// `muted`), clipped to 16 bits, as little-endian bytes.
pub fn finish(acc: &[i32], volume: u8, muted: bool, out: &mut [u8]) {
    let v = if muted { 0 } else { i32::from(volume.min(100)) };
    for (a, o) in acc.iter().zip(out.as_chunks_mut::<2>().0) {
        let s = (*a * v / 100).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        *o = s.to_le_bytes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(samples: &[i16]) -> alloc::vec::Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn mixing_adds_scales_and_clips() {
        let mut acc = [0i32; 4];
        mix_into(&mut acc, &bytes(&[1000, -1000, 30000, 0]), 100);
        mix_into(&mut acc, &bytes(&[500, 500, 30000, -20]), 50);
        assert_eq!(acc, [1250, -750, 45000, -10]);
        let mut out = [0u8; 8];
        finish(&acc, 100, false, &mut out);
        // 45000 clips to 32767.
        assert_eq!(out, bytes(&[1250, -750, 32767, -10]).as_slice());
        finish(&acc, 50, false, &mut out);
        assert_eq!(out, bytes(&[625, -375, 22500, -5]).as_slice());
        finish(&acc, 100, true, &mut out);
        assert_eq!(out, [0u8; 8]);
        // A short source leaves the rest alone.
        let mut acc = [7i32; 3];
        mix_into(&mut acc, &bytes(&[1]), 100);
        assert_eq!(acc, [8, 7, 7]);
    }

    #[test]
    fn keeps_the_lead() {
        let mut f = Feeder::new(65536, 100);
        assert_eq!(f.advance(100), LEAD);
        assert_eq!(f.piece(LEAD), (100, LEAD as usize));
        f.wrote(LEAD as usize);
        assert_eq!(f.advance(100), 0);
        // The controller moved 4 KiB: 4 KiB more to fill.
        assert_eq!(f.advance(100 + 4096), 4096);
    }

    #[test]
    fn wraps_around_the_buffer() {
        let mut f = Feeder::new(65536, 60_000);
        let want = f.advance(60_000);
        let (at, n) = f.piece(want);
        assert_eq!((at, n), (60_000, 5536));
        f.wrote(n);
        let (at, n) = f.piece(want - 5536);
        assert_eq!((at, n), (0, LEAD as usize - 5536));
        f.wrote(n);
        // LPIB wrapped to 1000: 6536 bytes played.
        assert_eq!(f.advance(1000), 6536);
        assert_eq!(f.hw, 60_000 + 6536);
    }

    #[test]
    fn late_thread_jumps_ahead() {
        let mut f = Feeder::new(65536, 0);
        f.wrote(1000);
        // The controller is past what was filled.
        assert_eq!(f.advance(30_000), LEAD);
        assert_eq!(f.fill, 30_000);
    }

    #[test]
    fn ring_bytes() {
        assert_eq!(from_ring(10, 4, 100), 6);
        assert_eq!(from_ring(10, 4, 3), 3);
        assert_eq!(from_ring(2, u32::MAX - 1, 100), 4);
        assert_eq!(from_ring(5, 5, 100), 0);
    }
}
