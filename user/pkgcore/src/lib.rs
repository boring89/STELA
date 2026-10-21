// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The package logic of `pkg` and Setup.app (P12, `docs/pkg.md`).
//!
//! ```text
//!  server (Data:/.stela/pkg.conf)            Data:/.pkg/
//!    index.json ─┐                             index.json      (verified copy)
//!    index.json.sig (ed25519, hex) ─► verify ─► installed.json  (name, version)
//!    NAME-VERSION-x86_64.tar ─► SHA-256 ─► unpack ─► Data:/Programs/NAME/
//!                                                     bin/ (on the shell's path)
//!                                                     Name.app/ (Mouseion sees it)
//! ```
//!
//! The pure parts (JSON, the index, ustar, dependency order, the
//! configuration, signatures and hashes) build on the host for tests;
//! `ops` does the work on STELA.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod json;
#[cfg(target_os = "none")]
pub mod ops;

use alloc::string::String;
use alloc::vec::Vec;
use json::Value;
use sha2::Digest;

/// Where packages go.
pub const PROGRAMS: &str = "Data:/Programs";
/// pkg's own files.
pub const STATE: &str = "Data:/.pkg";
/// The configuration (`server = URL`, `ca = Volume:path`).
pub const CONF: &str = "Data:/.stela/pkg.conf";
/// The public key the index must be signed with (hex, 32 bytes).
pub const PUBLIC_KEY: &str = "Initrd:/certs/pkg.pub";

/// One package of the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Package {
    /// Name (a folder name: letters, digits, `-`, `_`, `.`).
    pub name: String,
    /// Version text.
    pub version: String,
    /// One line about it.
    pub description: String,
    /// The program's license.
    pub license: String,
    /// `bin` or `app`.
    pub install: String,
    /// Packages it needs.
    pub depends: Vec<String>,
    /// Size of the tar file.
    pub size: u64,
    /// SHA-256 of the tar file (hex).
    pub sha256: String,
    /// Where to get it.
    pub url: String,
}

/// Why something was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PkgError {
    /// Not valid JSON, or a field missing.
    BadIndex,
    /// The signature does not match the index (or the key).
    BadSignature,
    /// The download does not have the SHA-256 the index gives.
    BadHash,
    /// A package name the index does not have.
    Unknown(String),
    /// Dependencies that go round in a circle.
    Cycle(String),
    /// A tar file that is broken or names a path outside its folder.
    BadArchive,
    /// A name that cannot be a folder.
    BadName(String),
}

/// Whether `name` is a safe package (folder) name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn text(v: &Value, key: &str) -> Result<String, PkgError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or(PkgError::BadIndex)
}

/// The packages of an `index.json` (`{"format": 1, "packages": [...]}`).
pub fn parse_index(data: &[u8]) -> Result<Vec<Package>, PkgError> {
    let text_ = core::str::from_utf8(data).map_err(|_| PkgError::BadIndex)?;
    let v = json::parse(text_).map_err(|_| PkgError::BadIndex)?;
    if v.get("format").and_then(Value::as_i64) != Some(1) {
        return Err(PkgError::BadIndex);
    }
    let list = v
        .get("packages")
        .and_then(Value::as_array)
        .ok_or(PkgError::BadIndex)?;
    let mut out = Vec::new();
    for p in list {
        let name = text(p, "name")?;
        if !valid_name(&name) {
            return Err(PkgError::BadName(name));
        }
        out.push(Package {
            name,
            version: text(p, "version")?,
            description: text(p, "description").unwrap_or_default(),
            license: text(p, "license").unwrap_or_default(),
            install: text(p, "install").unwrap_or_else(|_| String::from("bin")),
            depends: p
                .get("depends")
                .and_then(Value::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect(),
            size: p.get("size").and_then(Value::as_i64).unwrap_or(0) as u64,
            sha256: text(p, "sha256")?,
            url: text(p, "url")?,
        });
    }
    Ok(out)
}

/// Bytes from hex text (whitespace around ignored).
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    let t = text.trim();
    if !t.len().is_multiple_of(2) {
        return None;
    }
    (0..t.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(t.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Lower-case hex of `bytes`.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    for b in bytes {
        s.push_str(&alloc::format!("{b:02x}"));
    }
    s
}

/// SHA-256 of `data` (hex).
pub fn sha256_hex(data: &[u8]) -> String {
    to_hex(&sha2::Sha256::digest(data))
}

/// Whether `signature_hex` is `key_hex`'s ed25519 signature of `message`
/// (RFC 8032).
pub fn verify_signature(key_hex: &str, message: &[u8], signature_hex: &str) -> bool {
    let (Some(key), Some(sig)) = (from_hex(key_hex), from_hex(signature_hex)) else {
        return false;
    };
    let (Ok(key), Ok(sig)) = (<[u8; 32]>::try_from(key), <[u8; 64]>::try_from(sig)) else {
        return false;
    };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&key) else {
        return false;
    };
    key.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&sig))
        .is_ok()
}

/// The order to install `wanted` and what it needs: dependencies first,
/// each once. `have` are already installed (left out).
pub fn install_order(
    index: &[Package],
    wanted: &[&str],
    have: &[String],
) -> Result<Vec<Package>, PkgError> {
    fn visit(
        index: &[Package],
        name: &str,
        have: &[String],
        path: &mut Vec<String>,
        out: &mut Vec<Package>,
    ) -> Result<(), PkgError> {
        if out.iter().any(|p| p.name == name) || have.iter().any(|h| h == name) {
            return Ok(());
        }
        if path.iter().any(|p| p == name) {
            return Err(PkgError::Cycle(String::from(name)));
        }
        let p = index
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| PkgError::Unknown(String::from(name)))?;
        path.push(String::from(name));
        for d in &p.depends {
            visit(index, d, have, path, out)?;
        }
        path.pop();
        out.push(p.clone());
        Ok(())
    }
    let mut out = Vec::new();
    for name in wanted {
        visit(index, name, have, &mut Vec::new(), &mut out)?;
    }
    Ok(out)
}

/// One member of a ustar archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TarEntry<'a> {
    /// The path inside the archive (no `./`, no trailing `/`).
    pub path: String,
    /// A folder.
    pub is_dir: bool,
    /// The file's bytes.
    pub data: &'a [u8],
}

fn octal(field: &[u8]) -> Option<u64> {
    let t = core::str::from_utf8(field).ok()?;
    let t = t.trim_matches(|c: char| c == '\0' || c == ' ');
    if t.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(t, 8).ok()
}

/// The members of a ustar archive. Paths that would leave the package's
/// folder (absolute, `..`) are refused; links and devices are refused.
pub fn tar_entries(archive: &[u8]) -> Result<Vec<TarEntry<'_>>, PkgError> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 512 <= archive.len() {
        let h = &archive[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            return Ok(out);
        }
        let sum: u64 = h
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    u64::from(b)
                }
            })
            .sum();
        if octal(&h[148..156]) != Some(sum) {
            return Err(PkgError::BadArchive);
        }
        let cstr = |f: &[u8]| {
            let end = f.iter().position(|&b| b == 0).unwrap_or(f.len());
            core::str::from_utf8(&f[..end]).map(String::from)
        };
        let name = cstr(&h[0..100]).map_err(|_| PkgError::BadArchive)?;
        let prefix = cstr(&h[345..500]).map_err(|_| PkgError::BadArchive)?;
        let full = if prefix.is_empty() {
            name
        } else {
            alloc::format!("{prefix}/{name}")
        };
        let path = String::from(full.trim_start_matches("./").trim_end_matches('/'));
        let size = octal(&h[124..136]).ok_or(PkgError::BadArchive)? as usize;
        let kind = h[156];
        let start = at + 512;
        let end = start.checked_add(size).ok_or(PkgError::BadArchive)?;
        if end > archive.len() {
            return Err(PkgError::BadArchive);
        }
        if path.starts_with('/') || path.split('/').any(|p| p == "..") {
            return Err(PkgError::BadArchive);
        }
        match kind {
            b'0' | 0 => out.push(TarEntry {
                path,
                is_dir: false,
                data: &archive[start..end],
            }),
            b'5' => {
                if !path.is_empty() && path != "." {
                    out.push(TarEntry {
                        path,
                        is_dir: true,
                        data: &[],
                    });
                }
            }
            _ => return Err(PkgError::BadArchive),
        }
        at = start + size.next_multiple_of(512);
    }
    Ok(out)
}

/// The `PKGINFO` of a package archive: `key = value` lines.
pub fn pkginfo(entries: &[TarEntry<'_>]) -> Option<Package> {
    let info = entries.iter().find(|e| e.path == "PKGINFO")?;
    let text = core::str::from_utf8(info.data).ok()?;
    let mut p = Package::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = String::from(v.trim());
        match k.trim() {
            "name" => p.name = v,
            "version" => p.version = v,
            "description" => p.description = v,
            "license" => p.license = v,
            "install" => p.install = v,
            "depends" => p.depends = v.split_whitespace().map(String::from).collect(),
            _ => {}
        }
    }
    valid_name(&p.name).then_some(p)
}

/// The value of `key` in a `key = value` configuration text.
pub fn conf_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
}

/// What is installed: (name, version) in `installed.json`.
pub fn parse_installed(data: &[u8]) -> Vec<(String, String)> {
    let Some(v) = core::str::from_utf8(data)
        .ok()
        .and_then(|t| json::parse(t).ok())
    else {
        return Vec::new();
    };
    v.get("installed")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| {
            Some((
                String::from(p.get("name")?.as_str()?),
                String::from(p.get("version")?.as_str()?),
            ))
        })
        .collect()
}

/// `installed.json` for `list`.
pub fn installed_text(list: &[(String, String)]) -> String {
    let items = list
        .iter()
        .map(|(n, v)| json::object(alloc::vec![("name", json::s(n)), ("version", json::s(v))]))
        .collect();
    let mut t = json::object(alloc::vec![
        ("format", Value::Number(1)),
        ("installed", Value::Array(items)),
    ])
    .to_text();
    t.push('\n');
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const INDEX: &str = r#"{"format": 1, "packages": [
      {"name": "dash", "version": "0.5.12", "description": "a shell", "license": "BSD-3-Clause",
       "install": "bin", "depends": [], "size": 10, "sha256": "ab", "url": "https://x/dash.tar"},
      {"name": "make", "version": "4.4.1", "description": "make", "license": "GPL-3.0-or-later",
       "install": "bin", "depends": ["dash"], "size": 20, "sha256": "cd", "url": "https://x/make.tar"}
    ]}"#;

    #[test]
    fn index_and_order() {
        let idx = parse_index(INDEX.as_bytes()).unwrap();
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[1].depends, vec![String::from("dash")]);
        let order = install_order(&idx, &["make"], &[]).unwrap();
        let names: Vec<_> = order.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["dash", "make"]);
        let order = install_order(&idx, &["make"], &[String::from("dash")]).unwrap();
        assert_eq!(order.len(), 1);
        assert_eq!(
            install_order(&idx, &["nope"], &[]),
            Err(PkgError::Unknown(String::from("nope")))
        );
        let mut cyc = idx.clone();
        cyc[0].depends = vec![String::from("make")];
        assert!(matches!(
            install_order(&cyc, &["make"], &[]),
            Err(PkgError::Cycle(_))
        ));
        assert_eq!(parse_index(b"{\"format\": 2}"), Err(PkgError::BadIndex));
        let bad = INDEX.replace("\"dash\", \"version\"", "\"../x\", \"version\"");
        assert!(matches!(
            parse_index(bad.as_bytes()),
            Err(PkgError::BadName(_))
        ));
    }

    #[test]
    fn hex_hash_and_signature() {
        assert_eq!(from_hex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(to_hex(&[1, 0xab]), "01ab");
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // RFC 8032 §7.1, test 2 (one-byte message 0x72).
        let key = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";
        let sig = "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da\
                   085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00";
        assert!(verify_signature(key, &[0x72], sig));
        assert!(!verify_signature(key, &[0x73], sig));
        assert!(!verify_signature("00", &[0x72], sig));
    }

    fn header(name: &str, size: usize, kind: u8) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        let sz = alloc::format!("{size:011o}\0");
        h[124..136].copy_from_slice(sz.as_bytes());
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(alloc::format!("{sum:06o}\0 ").as_bytes());
        h
    }

    #[test]
    fn tar_reading() {
        let mut t = Vec::new();
        t.extend_from_slice(&header("PKGINFO", 22, b'0'));
        let mut data = b"name = lua\ndepends = \n".to_vec();
        data.resize(512, 0);
        t.extend_from_slice(&data);
        t.extend_from_slice(&header("bin/", 0, b'5'));
        t.extend_from_slice(&[0u8; 1024]);
        let e = tar_entries(&t).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].path, "bin");
        assert!(e[1].is_dir);
        let info = pkginfo(&e).unwrap();
        assert_eq!(info.name, "lua");
        assert!(info.depends.is_empty());
        let mut evil = Vec::new();
        evil.extend_from_slice(&header("../x", 0, b'0'));
        evil.extend_from_slice(&[0u8; 1024]);
        assert_eq!(tar_entries(&evil), Err(PkgError::BadArchive));
        let mut link = Vec::new();
        link.extend_from_slice(&header("l", 0, b'2'));
        assert_eq!(tar_entries(&link), Err(PkgError::BadArchive));
        let mut broken = t.clone();
        broken[0] = b'X';
        assert_eq!(tar_entries(&broken), Err(PkgError::BadArchive));
    }

    #[test]
    fn conf_and_installed() {
        let conf = "# pkg\nserver = https://10.0.2.2:8643/pkg/\nca = Data:/test-ca.der\n";
        assert_eq!(
            conf_value(conf, "server"),
            Some("https://10.0.2.2:8643/pkg/")
        );
        assert_eq!(conf_value(conf, "ca"), Some("Data:/test-ca.der"));
        assert_eq!(conf_value(conf, "x"), None);
        let list = vec![(String::from("lua"), String::from("5.4.7"))];
        let text = installed_text(&list);
        assert_eq!(parse_installed(text.as_bytes()), list);
        assert!(parse_installed(b"junk").is_empty());
        assert!(valid_name("rust-hello") && !valid_name("a/b") && !valid_name(".."));
    }
}
