// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Mac's sound to STELA's output (`docs/classic.md`): the Apple Sound
//! Chip plays unsigned 8-bit mono samples at about 22 kHz; the output
//! wants 48 kHz, 16-bit signed, stereo. Each output frame takes the input
//! sample under it (nearest neighbour), keeping the fraction between
//! calls so that the rate stays exact over time.

use alloc::vec::Vec;

/// The output rate.
pub const OUT_RATE: u32 = 48_000;

/// Converts a stream of 8-bit samples at one rate to output frames.
#[derive(Clone, Debug, Default)]
pub struct Resampler {
    /// Position of the next output frame in input samples, in units of
    /// 1/OUT_RATE.
    phase: u64,
}

impl Resampler {
    /// Converts `samples` (unsigned 8-bit, 128 = silence) at `rate` Hz.
    pub fn convert(&mut self, samples: &[u8], rate: u32) -> Vec<[i16; 2]> {
        let mut out = Vec::new();
        if rate == 0 || samples.is_empty() {
            return out;
        }
        let total = samples.len() as u64 * u64::from(OUT_RATE);
        while self.phase < total {
            let i = (self.phase / u64::from(OUT_RATE)) as usize;
            let v = (i16::from(samples[i]) - 128) * 256;
            out.push([v, v]);
            self.phase += u64::from(rate);
        }
        self.phase -= total;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_kept_across_calls() {
        let mut r = Resampler::default();
        let mut frames = 0;
        // One second at 22257 Hz, in pieces.
        for _ in 0..100 {
            frames += r.convert(&[128; 222], 22_200).len();
        }
        let expected = 22_200usize * 48_000 / 22_200;
        assert!((frames as i64 - expected as i64).abs() <= 1, "{frames}");
    }

    #[test]
    fn levels() {
        let mut r = Resampler::default();
        let out = r.convert(&[0, 128, 255], 48_000);
        assert_eq!(out, [[-32768, -32768], [0, 0], [32512, 32512]]);
        assert!(r.convert(&[], 22_000).is_empty());
        assert!(r.convert(&[1, 2], 0).is_empty());
    }
}
