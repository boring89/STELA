// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host tests of the trailer and the slot file.

use super::*;

extern crate std;

/// The test key's seed (`tools/pkgsign.py`: SHA-256 of a fixed
/// sentence); its public half is `tools/pkg.pub`.
fn test_signing_key() -> ed25519_dalek::SigningKey {
    let seed = sha256(b"STELA test package signing key - replace before use");
    ed25519_dalek::SigningKey::from_bytes(&seed)
}

fn signed(block: &mut [u8; BLOCK]) {
    use ed25519_dalek::Signer;
    let sig = test_signing_key().sign(&block[..SIGNED]);
    block[SIGNED..SIGNED + 64].copy_from_slice(&sig.to_bytes());
}

#[test]
fn trailer_round_trip_and_signature() {
    let data = [7u8; 10_000];
    let mut b = encode(10_000, &sha256(&data), "0.2.0").unwrap();
    let t = parse(&b).unwrap();
    assert_eq!(t.data_len, 10_000);
    assert_eq!(t.sha256, sha256(&data));
    assert_eq!(t.version, "0.2.0");
    let key = key_from_hex(include_str!("../../tools/pkg.pub")).unwrap();
    assert!(!verify(&b, &key), "unsigned");
    signed(&mut b);
    assert!(verify(&b, &key));
    // Any change to the signed part breaks it.
    let mut bad = b;
    bad[57] = b'9';
    assert!(!verify(&bad, &key));
    // The rest of the block is not signed.
    let mut tail = b;
    tail[BLOCK - 1] = 1;
    assert!(verify(&tail, &key));
    // Another key does not verify it.
    let mut other = key;
    other[0] ^= 1;
    assert!(!verify(&b, &other));
}

#[test]
fn trailer_rejects() {
    assert_eq!(parse(&[0u8; BLOCK]), None);
    assert_eq!(parse(&[0u8; 100]), None);
    let mut b = encode(1, &[0; 32], "x").unwrap();
    b[8] = 2;
    assert_eq!(parse(&b), None, "another format");
    assert!(encode(1, &[0; 32], &"x".repeat(65)).is_none());
    let mut b = encode(1, &[0; 32], "x").unwrap();
    b[56] = 200;
    assert_eq!(parse(&b), None);
    assert_eq!(key_from_hex("12"), None);
    assert_eq!(key_from_hex(&"zz".repeat(32)), None);
}

#[test]
fn hashing_in_pieces() {
    let data: std::vec::Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
    let mut h = Hasher::new();
    for piece in data.chunks(4096) {
        h.update(piece);
    }
    assert_eq!(h.finish(), sha256(&data));
}

#[test]
fn slot_file_text() {
    let f = SlotFile::parse("a 0\n").unwrap();
    assert_eq!(f, SlotFile::default());
    let f = SlotFile::parse("b 1 new").unwrap();
    assert_eq!(
        f,
        SlotFile {
            slot: Slot::B,
            tries: 1,
            fresh: true
        }
    );
    assert_eq!(f.text(), "b 1 new\n");
    assert_eq!(SlotFile::parse("b").unwrap().tries, 0);
    for bad in ["", "c 0", "a x", "a 0 old", "a 0 new more"] {
        assert_eq!(SlotFile::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn plans() {
    let file = |slot, tries, fresh| Some(SlotFile { slot, tries, fresh });
    // No file: A, signature only.
    assert_eq!(
        plan(None, None, false),
        Plan {
            first: Slot::A,
            full_check: false,
            gave_up: false
        }
    );
    // A fresh update or a start that did not reach the desktop: the
    // whole image is hashed.
    assert!(plan(file(Slot::B, 0, true), None, false).full_check);
    assert!(plan(file(Slot::B, 1, false), None, false).full_check);
    assert!(plan(None, None, true).full_check, "boot.cfg verify");
    // Two starts in a row did not reach the desktop: the other slot.
    let p = plan(file(Slot::B, 2, true), None, false);
    assert_eq!((p.first, p.gave_up), (Slot::A, true));
    // boot.cfg wins.
    assert_eq!(
        plan(file(Slot::B, 5, false), Some(Slot::B), false).first,
        Slot::B
    );
}

#[test]
fn starts_and_confirmation() {
    let fresh_b = SlotFile {
        slot: Slot::B,
        tries: 0,
        fresh: true,
    };
    // An update's first start: one try, still new.
    let f = after_start(Some(fresh_b), Slot::B, false).unwrap();
    assert_eq!(f.text(), "b 1 new\n");
    // It does not reach the desktop: the second try...
    let f = after_start(Some(f), Slot::B, false).unwrap();
    assert_eq!(f.tries, 2);
    // ...and the next start falls back to A.
    let p = plan(Some(f), None, false);
    assert_eq!(p.first, Slot::A);
    let f = after_start(Some(f), Slot::A, false).unwrap();
    assert_eq!(f.text(), "a 1\n");
    // The desktop came up.
    assert_eq!(confirmed(f).text(), "a 0\n");
    // A forced slot leaves the file alone.
    assert_eq!(after_start(Some(f), Slot::B, true), None);
}
