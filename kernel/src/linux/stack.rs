// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The initial stack of a Linux program (System V ABI AMD64, 3.4.1
//! "Initial Stack and Register State"). Pure logic, tested on the host.
//!
//! ```text
//!  top ─► "prog\0arg1\0" "VAR=x\0"     the strings
//!         16 random bytes               AT_RANDOM points here
//!         (padding to 16)
//!         AT_NULL 0                      ┐
//!         AT_xxx value ... AT_RANDOM ptr │ auxiliary vector (pairs)
//!         0                              │ end of envp
//!         envp[0..]                      │
//!         0                              │ end of argv
//!         argv[0..]                      │
//!  rsp ─► argc                           ┘ rsp is 16-byte aligned
//! ```

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Auxiliary vector types (`include/uapi/linux/auxvec.h`).
pub mod at {
    #![allow(missing_docs)]
    pub const NULL: u64 = 0;
    pub const PHDR: u64 = 3;
    pub const PHENT: u64 = 4;
    pub const PHNUM: u64 = 5;
    pub const PAGESZ: u64 = 6;
    pub const ENTRY: u64 = 9;
    pub const UID: u64 = 11;
    pub const EUID: u64 = 12;
    pub const GID: u64 = 13;
    pub const EGID: u64 = 14;
    pub const SECURE: u64 = 23;
    pub const RANDOM: u64 = 25;
}

/// The stack image: the bytes from `rsp` up to the top.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    /// The initial stack pointer (points at argc).
    pub rsp: u64,
    /// The bytes at `rsp..top`.
    pub bytes: Vec<u8>,
}

/// Builds the stack below `top` (16-byte aligned) for `args` and `env`,
/// with the auxiliary vector `aux` (AT_RANDOM and AT_NULL are added).
pub fn build(
    top: u64,
    args: &[&[u8]],
    env: &[&[u8]],
    aux: &[(u64, u64)],
    random: [u8; 16],
) -> Image {
    // Strings first, from the top down.
    let mut strings: Vec<u8> = Vec::new();
    let mut offsets = Vec::new();
    for s in args.iter().chain(env) {
        offsets.push(strings.len());
        strings.extend_from_slice(s);
        strings.push(0);
    }
    let strings_at = top - strings.len() as u64;
    let random_at = (strings_at - 16) & !15;
    let words = 1 + args.len() + 1 + env.len() + 1 + 2 * (aux.len() + 2);
    let rsp = (random_at - 8 * words as u64) & !15;
    let mut bytes = vec![0u8; (top - rsp) as usize];
    let at = |addr: u64| (addr - rsp) as usize;
    bytes[at(strings_at)..].copy_from_slice(&strings);
    bytes[at(random_at)..at(random_at) + 16].copy_from_slice(&random);
    let mut word = at(rsp);
    let mut put = |bytes: &mut Vec<u8>, value: u64| {
        bytes[word..word + 8].copy_from_slice(&value.to_le_bytes());
        word += 8;
    };
    put(&mut bytes, args.len() as u64);
    for (i, _) in args.iter().enumerate() {
        put(&mut bytes, strings_at + offsets[i] as u64);
    }
    put(&mut bytes, 0);
    for (i, _) in env.iter().enumerate() {
        put(&mut bytes, strings_at + offsets[args.len() + i] as u64);
    }
    put(&mut bytes, 0);
    for &(kind, value) in aux {
        put(&mut bytes, kind);
        put(&mut bytes, value);
    }
    put(&mut bytes, at::RANDOM);
    put(&mut bytes, random_at);
    put(&mut bytes, at::NULL);
    put(&mut bytes, 0);
    Image { rsp, bytes }
}

/// The interpreter line of a `#!` script (at most 255 bytes, like
/// Linux's `BINPRM_BUF_SIZE` limit): the interpreter and one optional
/// argument (the rest of the line, trimmed).
pub fn shebang(image: &[u8]) -> Option<(String, Option<String>)> {
    let rest = image.strip_prefix(b"#!")?;
    let end = rest
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(rest.len())
        .min(255);
    let line = core::str::from_utf8(&rest[..end]).ok()?.trim();
    let (interp, arg) = match line.find([' ', '\t']) {
        Some(i) => (&line[..i], Some(line[i..].trim())),
        None => (line, None),
    };
    if interp.is_empty() {
        return None;
    }
    let arg = arg.filter(|a| !a.is_empty()).map(String::from);
    Some((String::from(interp), arg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(image: &Image, addr: u64) -> u64 {
        let i = (addr - image.rsp) as usize;
        u64::from_le_bytes(image.bytes[i..i + 8].try_into().unwrap())
    }

    fn string(image: &Image, addr: u64) -> Vec<u8> {
        let i = (addr - image.rsp) as usize;
        image.bytes[i..]
            .iter()
            .take_while(|&&b| b != 0)
            .copied()
            .collect()
    }

    #[test]
    fn layout() {
        let top = 0x7FFF_FFF0_0000;
        let image = build(
            top,
            &[b"hello", b"a", b"bc"],
            &[b"HOME=/Data"],
            &[(at::PAGESZ, 4096), (at::ENTRY, 0x401000)],
            [7; 16],
        );
        assert_eq!(image.rsp % 16, 0);
        assert_eq!(image.rsp + image.bytes.len() as u64, top);
        let r = image.rsp;
        assert_eq!(word(&image, r), 3);
        assert_eq!(string(&image, word(&image, r + 8)), b"hello");
        assert_eq!(string(&image, word(&image, r + 16)), b"a");
        assert_eq!(string(&image, word(&image, r + 24)), b"bc");
        assert_eq!(word(&image, r + 32), 0);
        assert_eq!(string(&image, word(&image, r + 40)), b"HOME=/Data");
        assert_eq!(word(&image, r + 48), 0);
        let aux = r + 56;
        assert_eq!(
            (word(&image, aux), word(&image, aux + 8)),
            (at::PAGESZ, 4096)
        );
        assert_eq!(
            (word(&image, aux + 16), word(&image, aux + 24)),
            (at::ENTRY, 0x401000)
        );
        assert_eq!(word(&image, aux + 32), at::RANDOM);
        let random = word(&image, aux + 40);
        assert_eq!(random % 16, 0);
        let i = (random - r) as usize;
        assert_eq!(&image.bytes[i..i + 16], &[7; 16]);
        assert_eq!(
            (word(&image, aux + 48), word(&image, aux + 56)),
            (at::NULL, 0)
        );
    }

    #[test]
    fn no_arguments() {
        let image = build(0x10_0000, &[], &[], &[], [0; 16]);
        assert_eq!(word(&image, image.rsp), 0);
        assert_eq!(word(&image, image.rsp + 8), 0);
        assert_eq!(word(&image, image.rsp + 16), 0);
        assert_eq!(word(&image, image.rsp + 24), at::RANDOM);
    }

    #[test]
    fn shebang_lines() {
        assert_eq!(
            shebang(b"#!/bin/sh\necho hi\n"),
            Some((String::from("/bin/sh"), None))
        );
        assert_eq!(
            shebang(b"#! /usr/bin/env  lua -x \n"),
            Some((String::from("/usr/bin/env"), Some(String::from("lua -x"))))
        );
        assert_eq!(shebang(b"#!\n"), None);
        assert_eq!(shebang(b"\x7fELF"), None);
        assert_eq!(
            shebang(b"#!/bin/dash"),
            Some((String::from("/bin/dash"), None))
        );
    }
}
