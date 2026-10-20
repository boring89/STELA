// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sound output (`docs/sound.md`): 48 kHz, 16-bit signed, stereo samples
//! go into a ring shared with the kernel (`abi::audio`), which plays
//! them; while the ring is empty the output is silent.

use crate::ipc::{Mapping, handle_close};
use crate::sys::{syscall1, syscall3};
use abi::audio as ring;
use abi::{Error, Handle, Syscall};

/// The open output; closed when dropped.
pub struct Audio {
    handle: Handle,
    map: Mapping,
}

impl Audio {
    /// Opens the output (48 kHz, stereo); `Busy` if another program has
    /// it, `NotSupported` without a sound card.
    pub fn open() -> Result<Audio, Error> {
        let mut out = [0u64; 2];
        syscall3(
            Syscall::AudioOpen,
            u64::from(ring::RATE),
            u64::from(ring::CHANNELS),
            out.as_mut_ptr() as u64,
        )?;
        let handle = Handle::from_raw(out[0]);
        let memory = Handle::from_raw(out[1]);
        let map = Mapping::new(memory, ring::MEMORY_SIZE, true);
        // The mapping keeps the memory.
        let _ = handle_close(memory);
        match map {
            Ok(map) => Ok(Audio { handle, map }),
            Err(e) => {
                let _ = syscall1(Syscall::AudioClose, handle.raw());
                Err(e)
            }
        }
    }

    fn u32_at(&self, at: usize) -> u32 {
        let mut b = [0u8; 4];
        let _ = self.map.read(at as u64, &mut b);
        u32::from_le_bytes(b)
    }

    /// Bytes waiting in the ring (not yet taken by the kernel).
    pub fn queued(&self) -> u32 {
        self.u32_at(ring::WRITE_POS)
            .wrapping_sub(self.u32_at(ring::READ_POS))
    }

    /// Frames that fit in the ring now.
    pub fn writable(&self) -> usize {
        ((ring::RING_BYTES - self.queued()) / ring::FRAME_BYTES) as usize
    }

    /// Times the ring ran dry in the middle of the kernel's taking (the
    /// program fell behind; the end of the program's sound counts once).
    pub fn underruns(&self) -> u32 {
        self.u32_at(ring::UNDERRUNS)
    }

    /// Queues as many of `frames` (left, right pairs) as fit; returns how
    /// many.
    pub fn write(&self, frames: &[[i16; 2]]) -> usize {
        let n = frames.len().min(self.writable());
        let mut write = self.u32_at(ring::WRITE_POS);
        for f in &frames[..n] {
            let at = ring::DATA as u64 + u64::from(write % ring::RING_BYTES);
            let mut b = [0u8; 4];
            b[0..2].copy_from_slice(&f[0].to_le_bytes());
            b[2..4].copy_from_slice(&f[1].to_le_bytes());
            let _ = self.map.write(at, &b);
            write = write.wrapping_add(4);
        }
        // Publish after the samples are in place.
        let _ = self.map.write(ring::WRITE_POS as u64, &write.to_le_bytes());
        n
    }

    /// Writes all of `frames`, sleeping while the ring is full.
    pub fn write_all(&self, mut frames: &[[i16; 2]]) {
        while !frames.is_empty() {
            let n = self.write(frames);
            frames = &frames[n..];
            if !frames.is_empty() {
                crate::sleep_ms(10);
            }
        }
    }

    /// Waits until the kernel has taken everything queued (at most
    /// `timeout_ns`); the last ~85 ms are then still in the card.
    pub fn drain(&self, timeout_ns: u64) -> bool {
        let end = crate::now_ns().saturating_add(timeout_ns);
        while self.queued() > 0 {
            if crate::now_ns() >= end {
                return false;
            }
            crate::sleep_ms(5);
        }
        true
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        let _ = syscall1(Syscall::AudioClose, self.handle.raw());
    }
}

/// A square wave of `hz` at `amplitude` for `frames` frames starting at
/// frame `start` (for beeps and tests).
pub fn square(hz: u32, amplitude: i16, start: usize, frames: &mut [[i16; 2]]) {
    let period = (ring::RATE / hz.max(1)) as usize;
    for (i, f) in frames.iter_mut().enumerate() {
        let v = if ((start + i) % period) < period / 2 {
            amplitude
        } else {
            -amplitude
        };
        *f = [v, v];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_wave() {
        let mut f = [[0i16; 2]; 220];
        square(440, 1000, 0, &mut f);
        // 48000 / 440 = 109 frames per period: 54 high, 55 low.
        assert_eq!(f[0], [1000, 1000]);
        assert_eq!(f[53], [1000, 1000]);
        assert_eq!(f[54], [-1000, -1000]);
        assert_eq!(f[108], [-1000, -1000]);
        assert_eq!(f[109], [1000, 1000]);
    }
}
