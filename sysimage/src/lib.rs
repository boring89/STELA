// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The System image (P14 K5, `docs/system.md` §3–§5): its **trailer** and
//! the **slot file**. Pure logic, shared by the kernel (which checks an
//! image before mounting it and picks the slot), `stela-update` (which
//! checks an image before writing it) and the host tests.
//!
//! ```text
//!  System image:  [ eHFS volume (N blocks) ][ trailer: 1 block (4 KiB) ]
//!  trailer:       0   "STELASYS"
//!                 8   format (u32 LE, 1)
//!                 16  data length (u64 LE): the bytes before the trailer
//!                 24  SHA-256 of those bytes
//!                 56  version length (u8), 57.. the version (UTF-8, ≤ 64)
//!                 128 ed25519 signature of bytes 0..128
//!  slot file:     ESP:/STELA/slot, one line "a 0" or "b 1 new":
//!                 the slot, how many starts have not reached the desktop,
//!                 "new" until the first good start after an update
//! ```

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use sha2::Digest;

/// Bytes of the trailer (one eHFS block).
pub const BLOCK: usize = 4096;
/// The trailer's first bytes.
pub const MAGIC: &[u8; 8] = b"STELASYS";
/// The trailer's format.
pub const FORMAT: u32 = 1;
/// The signed part of the trailer: bytes `0..SIGNED`.
pub const SIGNED: usize = 128;
/// Longest version string.
pub const VERSION_MAX: usize = 64;
/// Starts that may fail (not reach the desktop) before the other slot is
/// taken.
pub const MAX_TRIES: u32 = 2;

/// What a trailer says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trailer {
    /// Bytes of the image before the trailer.
    pub data_len: u64,
    /// Their SHA-256.
    pub sha256: [u8; 32],
    /// The version (`0.2.0`).
    pub version: String,
}

/// SHA-256 over pieces fed one after another.
#[derive(Clone, Default)]
pub struct Hasher(sha2::Sha256);

impl Hasher {
    /// A new hash.
    pub fn new() -> Hasher {
        Hasher(sha2::Sha256::new())
    }
    /// Adds `data`.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    /// The hash.
    pub fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Hasher::new();
    h.update(data);
    h.finish()
}

/// The trailer for an image whose first `data_len` bytes hash to
/// `sha256`, unsigned (the signature bytes are zero); `None` when the
/// version is too long.
pub fn encode(data_len: u64, sha256: &[u8; 32], version: &str) -> Option<[u8; BLOCK]> {
    if version.len() > VERSION_MAX {
        return None;
    }
    let mut b = [0u8; BLOCK];
    b[..8].copy_from_slice(MAGIC);
    b[8..12].copy_from_slice(&FORMAT.to_le_bytes());
    b[16..24].copy_from_slice(&data_len.to_le_bytes());
    b[24..56].copy_from_slice(sha256);
    b[56] = version.len() as u8;
    b[57..57 + version.len()].copy_from_slice(version.as_bytes());
    Some(b)
}

/// The trailer in `block`, if it is one (not checking the signature).
pub fn parse(block: &[u8]) -> Option<Trailer> {
    if block.len() < BLOCK || &block[..8] != MAGIC {
        return None;
    }
    if u32::from_le_bytes(block[8..12].try_into().ok()?) != FORMAT {
        return None;
    }
    let n = usize::from(block[56]);
    if n > VERSION_MAX {
        return None;
    }
    let version = core::str::from_utf8(&block[57..57 + n]).ok()?;
    Some(Trailer {
        data_len: u64::from_le_bytes(block[16..24].try_into().ok()?),
        sha256: block[24..56].try_into().ok()?,
        version: String::from(version),
    })
}

/// Whether `block`'s signature is `key`'s (ed25519, RFC 8032) over its
/// first [`SIGNED`] bytes.
pub fn verify(block: &[u8], key: &[u8; 32]) -> bool {
    if block.len() < SIGNED + 64 {
        return false;
    }
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(key) else {
        return false;
    };
    let Ok(sig) = <[u8; 64]>::try_from(&block[SIGNED..SIGNED + 64]) else {
        return false;
    };
    key.verify_strict(
        &block[..SIGNED],
        &ed25519_dalek::Signature::from_bytes(&sig),
    )
    .is_ok()
}

/// A public key written in hex (`tools/system.pub`).
pub fn key_from_hex(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, k) in key.iter_mut().enumerate() {
        *k = u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(key)
}

/// One of the two System slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// The first STELA System partition.
    A,
    /// The second.
    B,
}

impl Slot {
    /// The other slot.
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
    /// `a` or `b`.
    pub fn letter(self) -> char {
        match self {
            Slot::A => 'a',
            Slot::B => 'b',
        }
    }
    /// The slot a letter names.
    pub fn from_letter(word: &str) -> Option<Slot> {
        match word {
            "a" | "A" => Some(Slot::A),
            "b" | "B" => Some(Slot::B),
            _ => None,
        }
    }
    /// 0 for A, 1 for B.
    pub fn index(self) -> usize {
        match self {
            Slot::A => 0,
            Slot::B => 1,
        }
    }
}

/// The slot file (`ESP:/STELA/slot`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotFile {
    /// The slot to start from.
    pub slot: Slot,
    /// Starts since the last one that reached the desktop (`BootOk`).
    pub tries: u32,
    /// Written by an update: not started well yet (check the whole image).
    pub fresh: bool,
}

impl Default for SlotFile {
    fn default() -> SlotFile {
        SlotFile {
            slot: Slot::A,
            tries: 0,
            fresh: false,
        }
    }
}

impl SlotFile {
    /// The file's text (`None` when it is not a slot file).
    pub fn parse(text: &str) -> Option<SlotFile> {
        let mut words = text.split_whitespace();
        let slot = Slot::from_letter(words.next()?)?;
        let tries = match words.next() {
            Some(w) => w.parse().ok()?,
            None => 0,
        };
        let fresh = match words.next() {
            Some("new") => true,
            Some(_) => return None,
            None => false,
        };
        if words.next().is_some() {
            return None;
        }
        Some(SlotFile { slot, tries, fresh })
    }

    /// The text to write.
    pub fn text(&self) -> String {
        let mut s = alloc::format!("{} {}", self.slot.letter(), self.tries);
        if self.fresh {
            s.push_str(" new");
        }
        s.push('\n');
        s
    }
}

/// Where a start begins (before the images are looked at).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The slot to try first (the other one is the fallback).
    pub first: Slot,
    /// Whether to hash the whole image, not only check the signature.
    pub full_check: bool,
    /// The slot file's slot failed [`MAX_TRIES`] starts in a row.
    pub gave_up: bool,
}

/// The plan for a start: `forced` is `boot.cfg`'s `system=a|b`, `verify`
/// its `verify` option, `file` the slot file (missing: `a 0`).
pub fn plan(file: Option<SlotFile>, forced: Option<Slot>, verify: bool) -> Plan {
    let file = file.unwrap_or_default();
    let unproven = file.fresh || file.tries > 0;
    if let Some(slot) = forced {
        return Plan {
            first: slot,
            full_check: verify || (slot == file.slot && unproven),
            gave_up: false,
        };
    }
    if file.tries >= MAX_TRIES {
        return Plan {
            first: file.slot.other(),
            full_check: true,
            gave_up: true,
        };
    }
    Plan {
        first: file.slot,
        full_check: verify || unproven,
        gave_up: false,
    }
}

/// The slot file after a start that mounted `mounted` (`None`: leave it,
/// as when `boot.cfg` forced a slot). A start counts as a try until
/// `BootOk` ([`confirmed`]).
pub fn after_start(file: Option<SlotFile>, mounted: Slot, forced: bool) -> Option<SlotFile> {
    if forced {
        return None;
    }
    let file = file.unwrap_or_default();
    Some(if mounted == file.slot {
        SlotFile {
            slot: mounted,
            tries: file.tries.saturating_add(1),
            fresh: file.fresh,
        }
    } else {
        // Fell back: the other slot, which had started well before.
        SlotFile {
            slot: mounted,
            tries: 1,
            fresh: false,
        }
    })
}

/// The slot file once the desktop is up (`BootOk`).
pub fn confirmed(file: SlotFile) -> SlotFile {
    SlotFile {
        slot: file.slot,
        tries: 0,
        fresh: false,
    }
}

#[cfg(test)]
mod tests;
