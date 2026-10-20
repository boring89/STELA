// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! File names (68k_HAL `docs/ehfs.md` sections 6 and 11.1, decisions E1
//! and E2): a name is 1 to 255 bytes of valid UTF-8 without NUL or `/`.
//! Two names are the same when their comparison forms are the same bytes:
//! Unicode Normalization Form C (UAX #15: canonical decomposition,
//! canonical ordering, canonical composition, with the Hangul syllable
//! algorithm), then simple case folding (CaseFolding.txt status C and S),
//! with the Unicode 15.0 tables of [`crate::unicode_data`].

use crate::unicode_data::{CCC, COMPOSE, DECOMP, DECOMP_CPS, FOLD};
use alloc::string::String;
use alloc::vec::Vec;

/// Longest name in bytes.
pub const NAME_MAX: usize = 255;
/// Longest comparison form (folding can make a name longer).
pub const KEY_MAX: usize = 1020;

// Hangul syllables (The Unicode Standard 15.0, section 3.12).
const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// Whether `name` may be used.
pub fn valid(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && core::str::from_utf8(name).is_ok_and(|s| !s.contains(['\0', '/']))
}

fn class(cp: u32) -> u8 {
    CCC.binary_search_by_key(&cp, |&(c, _)| c)
        .map_or(0, |i| CCC[i].1)
}

fn decompose(cp: u32, out: &mut Vec<u32>) {
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let s = cp - S_BASE;
        out.push(L_BASE + s / N_COUNT);
        out.push(V_BASE + (s % N_COUNT) / T_COUNT);
        if !s.is_multiple_of(T_COUNT) {
            out.push(T_BASE + s % T_COUNT);
        }
        return;
    }
    match DECOMP.binary_search_by_key(&cp, |&(c, _, _)| c) {
        Ok(i) => {
            let (_, at, len) = DECOMP[i];
            for &d in &DECOMP_CPS[usize::from(at)..usize::from(at) + usize::from(len)] {
                decompose(d, out);
            }
        }
        Err(_) => out.push(cp),
    }
}

/// The primary composite of `a` and `b`, if any.
fn composite(a: u32, b: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return Some(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE).is_multiple_of(T_COUNT)
        && b > T_BASE
        && b < T_BASE + T_COUNT
    {
        return Some(a + (b - T_BASE));
    }
    COMPOSE
        .binary_search_by_key(&(a, b), |&(x, y, _)| (x, y))
        .ok()
        .map(|i| COMPOSE[i].2)
}

fn fold(cp: u32) -> u32 {
    FOLD.binary_search_by_key(&cp, |&(f, _)| f)
        .map_or(cp, |i| FOLD[i].1)
}

/// NFC of `cps` (already fully decomposed), in place.
fn nfc(cps: &mut Vec<u32>) {
    // Canonical ordering: each run of non-starters sorted by class, stably.
    for i in 1..cps.len() {
        let c = class(cps[i]);
        let mut j = i;
        while j > 0 && c != 0 {
            if class(cps[j - 1]) <= c {
                break;
            }
            cps.swap(j, j - 1);
            j -= 1;
        }
    }
    if cps.is_empty() {
        return;
    }
    // Canonical composition (UAX #15's algorithm): a character joins the
    // last starter unless a character between them has class 0 or a class
    // not below its own. A leading non-starter means no starter yet (256).
    let mut starter = 0;
    let mut out = 1;
    let mut last: u32 = if class(cps[0]) == 0 { 0 } else { 256 };
    for i in 1..cps.len() {
        let c = cps[i];
        let cls = u32::from(class(c));
        if let Some(comp) = composite(cps[starter], c)
            && (last < cls || last == 0)
        {
            cps[starter] = comp;
            continue;
        }
        if cls == 0 {
            starter = out;
        }
        last = cls;
        cps[out] = c;
        out += 1;
    }
    cps.truncate(out);
}

/// The NFC form of `name` (no case folding), if it is valid.
pub fn normalize(name: &str) -> String {
    let mut cps = Vec::new();
    for c in name.chars() {
        decompose(u32::from(c), &mut cps);
    }
    nfc(&mut cps);
    cps.iter().filter_map(|&c| char::from_u32(c)).collect()
}

/// The comparison form of `name` as UTF-8, or `None` if the name is not
/// valid (or its form is longer than [`KEY_MAX`]).
pub fn key(name: &[u8]) -> Option<Vec<u8>> {
    if !valid(name) {
        return None;
    }
    let s = core::str::from_utf8(name).ok()?;
    let mut cps = Vec::with_capacity(s.len());
    for c in s.chars() {
        decompose(u32::from(c), &mut cps);
    }
    nfc(&mut cps);
    let mut out = Vec::with_capacity(cps.len());
    for &c in &cps {
        let ch = char::from_u32(fold(c))?;
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    (out.len() <= KEY_MAX).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: &str, b: &str) -> bool {
        key(a.as_bytes()) == key(b.as_bytes())
    }

    #[test]
    fn validity() {
        assert!(valid(b"Read Me.txt"));
        assert!(!valid(b""));
        assert!(!valid(b"a/b"));
        assert!(!valid(b"a\0b"));
        assert!(!valid(&[0xC0, 0x80])); // overlong NUL
        assert!(!valid(&[0xED, 0xA0, 0x80])); // a surrogate
        assert!(valid(&[b'a'; 255]));
        assert!(!valid(&[b'a'; 256]));
    }

    #[test]
    fn case_and_normalization() {
        assert!(same("READ ME.TXT", "Read Me.txt"));
        assert!(!same("Read Me.txt", "ReadMe.txt"));
        // NFC: e + combining acute = é; and the case of É folds to é.
        assert!(same("Caf\u{65}\u{301}", "café"));
        assert!(same("CAFÉ", "café"));
        assert_eq!(
            key("Caf\u{65}\u{301}".as_bytes()).unwrap(),
            "café".as_bytes()
        );
        // Greek final sigma folds to sigma; capital sigma too.
        assert!(same("ΣΟΦΙΑ", "σοφια"));
        assert!(same("ς", "σ"));
        // Simple folding: ß stays ß (no "ss").
        assert!(!same("Straße", "Strasse"));
        // Hangul: a precomposed syllable equals its jamo.
        assert!(same("\u{D55C}", "\u{1112}\u{1161}\u{11AB}"));
        // Ordering of combining marks: dot below (220) before acute (230).
        assert!(same("a\u{301}\u{323}", "a\u{323}\u{301}"));
        assert_eq!(normalize("e\u{301}"), "é");
    }

    #[test]
    fn unicode_normalization_test_if_present() {
        // The official NormalizationTest.txt (Unicode 15.0) checks NFC on
        // ~19,000 lines; run it when EHFS_NORMALIZATION_TEST names the file.
        let Ok(path) = std::env::var("EHFS_NORMALIZATION_TEST") else {
            return;
        };
        let text = std::fs::read_to_string(path).unwrap();
        let mut checked = 0;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("");
            if line.is_empty() || line.starts_with('@') {
                continue;
            }
            let cols: Vec<String> = line
                .split(';')
                .take(5)
                .map(|c| {
                    c.split_whitespace()
                        .map(|h| char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap())
                        .collect()
                })
                .collect();
            // NFC(c1..c3) == c2, NFC(c4, c5) == c4.
            for (src, want) in [(0, 1), (1, 1), (2, 1), (3, 3), (4, 3)] {
                assert_eq!(normalize(&cols[src]), cols[want], "line {line}");
                checked += 1;
            }
        }
        assert!(checked > 90_000);
    }
}
