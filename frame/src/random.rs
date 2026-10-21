// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Random bytes for the kernel and for programs (Linux `getrandom`, the
//! `Random` system call, `AT_RANDOM`; P12, `docs/tls.md` §2).
//!
//! ```text
//!  entropy: RDSEED / RDRAND (CPUID.07H:EBX[18] / CPUID.01H:ECX[30])
//!           or, without them, the jitter of the TSC over a busy loop
//!        │ 32 bytes, at the first use and after every 1 MiB
//!        ▼
//!  ChaCha20 key ──► blocks (RFC 8439 §2.3) ──► the caller's bytes
//!        ▲                       │
//!        └─ the next block's first 32 bytes after every request
//!           (fast key erasure: an old key cannot be recovered)
//! ```
//!
//! The ChaCha20 block function is pure and tested on the host with the
//! RFC's test vector.

/// The ChaCha20 quarter round (RFC 8439 §2.1).
fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// One 64-byte ChaCha20 block for `key`, block `counter` and `nonce`
/// (RFC 8439 §2.3).
pub fn chacha20_block(key: &[u32; 8], counter: u32, nonce: &[u32; 3]) -> [u8; 64] {
    let mut init = [0u32; 16];
    init[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    init[4..12].copy_from_slice(key);
    init[12] = counter;
    init[13..].copy_from_slice(nonce);
    let mut s = init;
    for _ in 0..10 {
        quarter(&mut s, 0, 4, 8, 12);
        quarter(&mut s, 1, 5, 9, 13);
        quarter(&mut s, 2, 6, 10, 14);
        quarter(&mut s, 3, 7, 11, 15);
        quarter(&mut s, 0, 5, 10, 15);
        quarter(&mut s, 1, 6, 11, 12);
        quarter(&mut s, 2, 7, 8, 13);
        quarter(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[i * 4..i * 4 + 4].copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
    out
}

/// A ChaCha20 generator: a key and a block counter (the nonce is zero;
/// the key changes after every request).
#[derive(Clone)]
pub struct Generator {
    key: [u32; 8],
    counter: u64,
    /// Bytes given since the last reseed.
    since_seed: u64,
}

/// Bytes after which the key is mixed with fresh entropy.
pub const RESEED_AFTER: u64 = 1 << 20;

impl Generator {
    /// A generator keyed with `seed`.
    pub fn new(seed: &[u8; 32]) -> Generator {
        let mut g = Generator {
            key: [0; 8],
            counter: 0,
            since_seed: 0,
        };
        g.mix(seed);
        g
    }

    /// Mixes `seed` into the key (XOR, then one block to spread it).
    pub fn mix(&mut self, seed: &[u8; 32]) {
        for (i, k) in self.key.iter_mut().enumerate() {
            *k ^= u32::from_le_bytes([
                seed[i * 4],
                seed[i * 4 + 1],
                seed[i * 4 + 2],
                seed[i * 4 + 3],
            ]);
        }
        self.rekey();
        self.since_seed = 0;
    }

    /// Replaces the key with the next block's first 32 bytes.
    fn rekey(&mut self) {
        let block = self.block();
        for (i, k) in self.key.iter_mut().enumerate() {
            *k = u32::from_le_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
    }

    fn block(&mut self) -> [u8; 64] {
        let counter = self.counter;
        self.counter = self.counter.wrapping_add(1);
        chacha20_block(&self.key, counter as u32, &[(counter >> 32) as u32, 0, 0])
    }

    /// Fills `buf`, then changes the key.
    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(64) {
            let block = self.block();
            chunk.copy_from_slice(&block[..chunk.len()]);
        }
        self.since_seed = self.since_seed.saturating_add(buf.len() as u64);
        self.rekey();
    }

    /// Whether the generator should get fresh entropy.
    pub fn needs_seed(&self) -> bool {
        self.since_seed >= RESEED_AFTER
    }
}

#[cfg(target_os = "none")]
mod hw {
    use super::Generator;
    use crate::sync::SpinLock;
    use core::arch::asm;
    use core::arch::x86_64::{__cpuid, __cpuid_count};

    static GENERATOR: SpinLock<Option<Generator>> = SpinLock::new(None);

    /// Where the entropy comes from.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Source {
        /// RDSEED (Intel SDM Vol. 2B "RDSEED").
        Rdseed,
        /// RDRAND (Vol. 2B "RDRAND").
        Rdrand,
        /// TSC jitter.
        Jitter,
    }

    /// The best source this processor has.
    pub fn source() -> Source {
        let max = __cpuid(0).eax;
        if max >= 7 && __cpuid_count(7, 0).ebx & (1 << 18) != 0 {
            Source::Rdseed
        } else if __cpuid(1).ecx & (1 << 30) != 0 {
            Source::Rdrand
        } else {
            Source::Jitter
        }
    }

    fn rdtsc() -> u64 {
        let (low, high): (u32, u32);
        // SAFETY: `rdtsc` only reads the time-stamp counter (Vol. 2B
        // "RDTSC"); CR4.TSD is clear.
        unsafe {
            asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags))
        };
        (u64::from(high) << 32) | u64::from(low)
    }

    /// One 64-bit value from RDSEED or RDRAND; `None` after 10 failures
    /// (CF = 0 means "no value yet": Intel's "Digital Random Number
    /// Generator Software Implementation Guide" §5.2.1 retries 10 times).
    fn hardware(seed: bool) -> Option<u64> {
        for _ in 0..10 {
            let value: u64;
            let ok: u8;
            // SAFETY: the instruction exists (checked by `source`); it
            // only writes the output register and CF.
            unsafe {
                if seed {
                    asm!("rdseed {v}", "setc {ok}", v = out(reg) value, ok = out(reg_byte) ok,
                         options(nomem, nostack));
                } else {
                    asm!("rdrand {v}", "setc {ok}", v = out(reg) value, ok = out(reg_byte) ok,
                         options(nomem, nostack));
                }
            }
            if ok != 0 {
                return Some(value);
            }
            core::hint::spin_loop();
        }
        None
    }

    /// 32 bytes of entropy: hardware values, or the low bits of TSC
    /// differences over a loop whose time varies (caches, interrupts,
    /// the processor's clocking), many samples per output bit.
    fn entropy() -> [u8; 32] {
        let mut out = [0u8; 32];
        let src = source();
        if src != Source::Jitter {
            let mut all = true;
            for chunk in out.chunks_mut(8) {
                match hardware(src == Source::Rdseed).or_else(|| hardware(false)) {
                    Some(v) => chunk.copy_from_slice(&v.to_le_bytes()),
                    None => all = false,
                }
            }
            if all {
                return out;
            }
        }
        // 4096 samples folded into 32 bytes: each sample's low byte is
        // mixed through a running ChaCha block every 64 samples.
        let mut key = [0u32; 8];
        let mut acc = [0u8; 64];
        let mut last = rdtsc();
        let mut sink = 0u64;
        for i in 0..4096usize {
            let spins = (last & 0x3F) + 8;
            for k in 0..spins {
                sink = sink.wrapping_mul(6364136223846793005).wrapping_add(k);
            }
            let now = rdtsc();
            acc[i % 64] ^= (now.wrapping_sub(last) as u8) ^ (sink as u8);
            last = now;
            if i % 64 == 63 {
                for (j, k) in key.iter_mut().enumerate() {
                    *k ^= u32::from_le_bytes([
                        acc[j * 4],
                        acc[j * 4 + 1],
                        acc[j * 4 + 2],
                        acc[j * 4 + 3],
                    ]);
                }
                let block = super::chacha20_block(&key, i as u32, &[0, 0, 0]);
                acc.copy_from_slice(&block);
            }
        }
        out.copy_from_slice(&acc[..32]);
        out
    }

    /// Fills `buf` with random bytes.
    pub fn fill(buf: &mut [u8]) {
        let mut g = GENERATOR.lock();
        match g.as_mut() {
            Some(generator) if !generator.needs_seed() => generator.fill(buf),
            Some(generator) => {
                generator.mix(&entropy());
                generator.fill(buf);
            }
            None => {
                let mut generator = Generator::new(&entropy());
                generator.fill(buf);
                *g = Some(generator);
            }
        }
    }
}

#[cfg(target_os = "none")]
pub use hw::{Source, fill, source};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc8439_block_test_vector() {
        // RFC 8439 §2.3.2.
        let mut key = [0u32; 8];
        for (i, k) in key.iter_mut().enumerate() {
            let b = (i * 4) as u8;
            *k = u32::from_le_bytes([b, b + 1, b + 2, b + 3]);
        }
        let nonce = [0x0900_0000, 0x4a00_0000, 0];
        let out = chacha20_block(&key, 1, &nonce);
        assert_eq!(
            &out[..16],
            &[
                0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3, 0x20,
                0x71, 0xc4
            ]
        );
        assert_eq!(&out[60..], &[0xa2, 0x50, 0x3c, 0x4e]);
    }

    #[test]
    fn generator_changes_key_and_differs() {
        let mut g = Generator::new(&[7; 32]);
        let mut a = [0u8; 100];
        let mut b = [0u8; 100];
        g.fill(&mut a);
        g.fill(&mut b);
        assert_ne!(a, b);
        let mut h = Generator::new(&[7; 32]);
        let mut c = [0u8; 100];
        h.fill(&mut c);
        assert_eq!(a, c);
        let mut other = Generator::new(&[8; 32]);
        other.fill(&mut c);
        assert_ne!(a, c);
        assert!(!g.needs_seed());
        let mut big = alloc::vec![0u8; RESEED_AFTER as usize];
        g.fill(&mut big);
        assert!(g.needs_seed());
    }
}
