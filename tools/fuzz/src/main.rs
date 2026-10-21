// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host fuzzing (P11 K45, `docs/testing.md` §3): every parser that reads
//! data STELA does not control gets valid inputs changed at random, and
//! must not panic (a slice index out of bounds, an overflow, an
//! `unwrap`) or run forever.
//!
//! ```text
//!  seed (a valid input) ──► mutate: flip bits, random and boundary bytes,
//!                           cut, repeat, insert, delete, copy (1–8 times)
//!                       ──► target(input) inside catch_unwind
//!  a watchdog thread: no progress for 10 s → "stuck", the input is saved
//!  a panic → the input is saved to build/fuzz-<target>.bin, exit 1
//! ```
//!
//! Disk images (eHFS, FAT, GPT) are not copied: an [`Overlay`] keeps the
//! changed blocks over the seed image, and only blocks the seed's run
//! read are changed (the metadata). The network target also lets two
//! stacks talk while frames are changed, dropped and repeated on the way.
//! The random numbers start from a fixed seed: a failure repeats.
//!
//! Usage: `fuzz BUILD_DIR [TIME_PERCENT [TARGET]]` (`make test-fuzz`):
//! every target stops after its inputs or its time (scaled by
//! TIME_PERCENT, 100 by default; at most 60 s in all); TARGET runs only
//! that one.

use block::BlockDevice;
use block::BlockError;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

mod targets;

/// Bumped after every input: the watchdog's sign of life.
static PROGRESS: AtomicU64 = AtomicU64::new(0);
/// The target running and its input (saved if it hangs).
static CURRENT: Mutex<(&str, Vec<u8>)> = Mutex::new(("", Vec::new()));

/// A small fast random number generator (xorshift64*).
pub struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    /// The next number.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    /// True once in `n`.
    pub fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
}

/// Values that sit on the edges parsers check.
const INTERESTING: [u64; 12] = [
    0,
    1,
    2,
    0x7F,
    0x80,
    0xFF,
    0x100,
    0x7FFF,
    0xFFFF,
    0x8000_0000,
    0xFFFF_FFFF,
    u64::MAX,
];

/// Changes `data` in place without changing its length.
pub fn mutate_in_place(rng: &mut Rng, data: &mut [u8]) {
    if data.is_empty() {
        return;
    }
    let at = rng.below(data.len());
    match rng.below(4) {
        0 => data[at] ^= 1 << rng.below(8),
        1 => data[at] = rng.next_u64() as u8,
        2 => {
            let v = INTERESTING[rng.below(INTERESTING.len())];
            let width = [1, 2, 4, 8][rng.below(4)];
            let bytes = if rng.one_in(2) {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            };
            for i in 0..width.min(data.len() - at) {
                data[at + i] = if rng.one_in(2) {
                    bytes[i]
                } else {
                    bytes[8 - width + i]
                };
            }
        }
        _ => {
            let len = rng.below(16).min(data.len() - at);
            let from = rng.below(data.len() - len + 1);
            data.copy_within(from..from + len, at);
        }
    }
}

/// 1–8 changes to `data`, its length included (at most `max` bytes).
pub fn mutate(rng: &mut Rng, data: &mut Vec<u8>, max: usize) {
    for _ in 0..1 + rng.below(8) {
        match rng.below(8) {
            0..=3 => mutate_in_place(rng, data),
            4 => data.truncate(rng.below(data.len() + 1)),
            5 => {
                let at = rng.below(data.len() + 1);
                let n = 1 + rng.below(16);
                let bytes: Vec<u8> = (0..n).map(|_| rng.next_u64() as u8).collect();
                data.splice(at..at, bytes);
            }
            6 if !data.is_empty() => {
                let from = rng.below(data.len());
                let len = (1 + rng.below(64)).min(data.len() - from);
                let chunk = data[from..from + len].to_vec();
                let at = rng.below(data.len() + 1);
                data.splice(at..at, chunk);
            }
            _ if !data.is_empty() => {
                let from = rng.below(data.len());
                let len = (1 + rng.below(32)).min(data.len() - from);
                data.drain(from..from + len);
            }
            _ => data.push(rng.next_u64() as u8),
        }
    }
    data.truncate(max);
}

/// A disk image with changed blocks over a seed image; records the
/// blocks read.
pub struct Overlay<'a> {
    base: &'a [u8],
    changed: RefCell<BTreeMap<u64, Vec<u8>>>,
    reads: RefCell<BTreeSet<u64>>,
}

/// Bytes in a block.
pub const BLOCK: usize = block::BLOCK_SIZE;

impl<'a> Overlay<'a> {
    /// The image `base` unchanged.
    pub fn new(base: &'a [u8]) -> Overlay<'a> {
        Overlay {
            base,
            changed: RefCell::new(BTreeMap::new()),
            reads: RefCell::new(BTreeSet::new()),
        }
    }

    /// The blocks read so far.
    pub fn reads(&self) -> Vec<u64> {
        self.reads.borrow().iter().copied().collect()
    }

    /// Block `n` to change (a copy of the seed's the first time).
    pub fn with_block<R>(&self, n: u64, f: impl FnOnce(&mut [u8]) -> R) -> R {
        let mut changed = self.changed.borrow_mut();
        let b = changed.entry(n).or_insert_with(|| {
            let at = n as usize * BLOCK;
            let mut v = vec![0u8; BLOCK];
            if at < self.base.len() {
                let end = (at + BLOCK).min(self.base.len());
                v[..end - at].copy_from_slice(&self.base[at..end]);
            }
            v
        });
        f(b)
    }

    /// The numbers of the changed blocks.
    pub fn changed_blocks(&self) -> Vec<u64> {
        self.changed.borrow().keys().copied().collect()
    }

    /// The changed blocks as bytes (block number, then the block): what
    /// is saved when an input fails.
    pub fn changes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (n, b) in self.changed.borrow().iter() {
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(b);
        }
        out
    }

    /// Changes 1–8 places in the blocks `hot`.
    pub fn mutate(&self, rng: &mut Rng, hot: &[u64]) {
        if hot.is_empty() {
            return;
        }
        for _ in 0..1 + rng.below(8) {
            let n = hot[rng.below(hot.len())];
            self.with_block(n, |b| mutate_in_place(rng, b));
        }
    }
}

impl BlockDevice for Overlay<'_> {
    fn block_count(&self) -> u64 {
        self.base.len().div_ceil(BLOCK) as u64
    }

    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.is_empty() || !buf.len().is_multiple_of(BLOCK) {
            return Err(BlockError::BadBuffer);
        }
        let count = (buf.len() / BLOCK) as u64;
        if block
            .checked_add(count)
            .is_none_or(|end| end > self.block_count())
        {
            return Err(BlockError::OutOfRange);
        }
        for (i, chunk) in buf.chunks_mut(BLOCK).enumerate() {
            let n = block + i as u64;
            self.reads.borrow_mut().insert(n);
            if let Some(b) = self.changed.borrow().get(&n) {
                chunk.copy_from_slice(b);
                continue;
            }
            let at = n as usize * BLOCK;
            let end = (at + BLOCK).min(self.base.len());
            chunk.fill(0);
            chunk[..end - at].copy_from_slice(&self.base[at..end]);
        }
        Ok(())
    }

    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.is_empty() || !buf.len().is_multiple_of(BLOCK) {
            return Err(BlockError::BadBuffer);
        }
        let count = (buf.len() / BLOCK) as u64;
        if block
            .checked_add(count)
            .is_none_or(|end| end > self.block_count())
        {
            return Err(BlockError::OutOfRange);
        }
        for (i, chunk) in buf.chunks(BLOCK).enumerate() {
            self.with_block(block + i as u64, |b| b.copy_from_slice(chunk));
        }
        Ok(())
    }
}

/// A fuzz target: a name, how many inputs, and one run (the input it
/// tried goes into `CURRENT` first).
pub struct Target {
    /// Its name (`make test-fuzz` prints it).
    pub name: &'static str,
    /// Inputs to try.
    pub inputs: u64,
    /// Seconds it may take at most (fewer inputs on a slow machine).
    pub seconds: u64,
    /// One input: what to try, from the random numbers.
    pub run: Box<dyn FnMut(&mut Rng)>,
}

/// Saves the input of the current run.
pub fn note(input: &[u8]) {
    let mut c = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    c.1.clear();
    c.1.extend_from_slice(input);
}

fn save(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let path = dir.join(format!("fuzz-{name}.bin"));
    if let Err(e) = std::fs::write(&path, data) {
        eprintln!("fuzz: cannot save {}: {e}", path.display());
    }
    path
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(build) = args.get(1).map(PathBuf::from) else {
        eprintln!("usage: fuzz BUILD_DIR [TIME_PERCENT [TARGET]]");
        std::process::exit(2);
    };
    let percent: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100);
    let only = args.get(3);
    let watch_dir = build.clone();
    std::thread::spawn(move || {
        let mut last = u64::MAX;
        let mut still = 0;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = PROGRESS.load(Ordering::Relaxed);
            still = if now == last { still + 1 } else { 0 };
            last = now;
            if still >= 10 {
                let c = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
                let path = save(&watch_dir, c.0, &c.1);
                eprintln!(
                    "fuzz: {}: no progress for 10 s (an endless loop?); input in {}",
                    c.0,
                    path.display()
                );
                std::process::exit(1);
            }
        }
    });
    let started = Instant::now();
    for mut t in targets::all(&build) {
        if only.is_some_and(|o| o != t.name) {
            continue;
        }
        let limit = Duration::from_millis(t.seconds * percent * 10);
        CURRENT.lock().unwrap_or_else(|e| e.into_inner()).0 = t.name;
        // FUZZ_SEED changes the inputs (to look further by hand).
        let extra: u64 = std::env::var("FUZZ_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut rng = Rng::new(0x5745_4C41_0000_0000 ^ t.name.len() as u64 ^ extra << 32);
        let begun = Instant::now();
        let mut done = 0;
        while done < t.inputs && begun.elapsed() < limit {
            let r = catch_unwind(AssertUnwindSafe(|| (t.run)(&mut rng)));
            if r.is_err() {
                let c = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
                let path = save(&build, t.name, &c.1);
                eprintln!(
                    "fuzz: {}: panicked on input {done}; input in {}",
                    t.name,
                    path.display()
                );
                std::process::exit(1);
            }
            done += 1;
            PROGRESS.fetch_add(1, Ordering::Relaxed);
        }
        println!(
            "fuzz: {:<8} {done:>7} inputs in {:.1} s",
            t.name,
            begun.elapsed().as_secs_f64()
        );
    }
    println!("fuzz: OK ({:.1} s)", started.elapsed().as_secs_f64());
}
