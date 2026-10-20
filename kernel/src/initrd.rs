// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading the initial RAM disk: a ustar archive (POSIX.1-2001 "pax"
//! interchange format, the ustar header; GNU tar `--format=ustar` writes
//! it).
//!
//! ```text
//!  512-byte header   offset  size  field
//!                    0       100   name (NUL-terminated if shorter)
//!                    124     12    size, octal ASCII
//!                    148     8     checksum: sum of all header bytes, with
//!                                  this field counted as 8 spaces; octal
//!                    156     1     type: '0' or NUL = file, '5' = directory
//!                    257     6     magic "ustar\0" (GNU: "ustar ")
//!                    345     155   prefix: path = prefix + "/" + name
//!  then the file's bytes, padded to a multiple of 512
//!  ...
//!  two all-zero blocks end the archive
//! ```

/// Size of a header and of the padding unit.
const BLOCK: usize = 512;

/// What a member is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// Anything else (links, devices...), skipped by [`find`].
    Other,
}

/// One member of the archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The path without a leading `./`, `prefix` and `name` joined; split
    /// in two when the prefix is used (`prefix`, then `name`).
    pub prefix: &'a str,
    /// See [`Entry::prefix`].
    pub name: &'a str,
    /// The member type.
    pub kind: Kind,
    /// The member's bytes.
    pub data: &'a [u8],
}

impl Entry<'_> {
    /// Whether the member's path is `path`.
    pub fn is(&self, path: &str) -> bool {
        if self.prefix.is_empty() {
            self.name == path
        } else {
            path.len() == self.prefix.len() + 1 + self.name.len()
                && path.starts_with(self.prefix)
                && path.as_bytes()[self.prefix.len()] == b'/'
                && path.ends_with(self.name)
        }
    }
}

/// Why the archive could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A header or the data it announces runs past the end.
    Truncated,
    /// A header's magic is not ustar.
    BadMagic,
    /// A header's checksum does not match.
    BadChecksum,
    /// A number field is not octal, or a name is not UTF-8.
    BadField,
}

/// Parses an octal ASCII field (spaces and NULs end or pad it).
fn octal(field: &[u8]) -> Result<u64, Error> {
    let mut value: u64 = 0;
    let mut digits = 0;
    for &b in field {
        match b {
            b'0'..=b'7' => {
                value = value
                    .checked_mul(8)
                    .and_then(|v| v.checked_add(u64::from(b - b'0')))
                    .ok_or(Error::BadField)?;
                digits += 1;
            }
            b' ' | 0 if digits == 0 => {}
            b' ' | 0 => break,
            _ => return Err(Error::BadField),
        }
    }
    Ok(value)
}

/// A NUL-terminated (or full-length) text field.
fn text(field: &[u8]) -> Result<&str, Error> {
    let len = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    core::str::from_utf8(&field[..len]).map_err(|_| Error::BadField)
}

/// Iterates over the members of a ustar archive.
#[derive(Clone, Debug)]
pub struct Entries<'a> {
    rest: &'a [u8],
    done: bool,
}

/// The members of `archive`, in order. An empty archive (no bytes, or only
/// the end marker) has none.
pub fn entries(archive: &[u8]) -> Entries<'_> {
    Entries {
        rest: archive,
        done: false,
    }
}

impl<'a> Entries<'a> {
    fn parse_one(&mut self) -> Result<Option<Entry<'a>>, Error> {
        if self.rest.is_empty() {
            return Ok(None);
        }
        let header = self.rest.get(..BLOCK).ok_or(Error::Truncated)?;
        if header.iter().all(|&b| b == 0) {
            return Ok(None);
        }
        if &header[257..262] != b"ustar" {
            return Err(Error::BadMagic);
        }
        let stored = octal(&header[148..156])?;
        let sum: u64 = header
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                if (148..156).contains(&i) {
                    u64::from(b' ')
                } else {
                    u64::from(b)
                }
            })
            .sum();
        if sum != stored {
            return Err(Error::BadChecksum);
        }
        let size = usize::try_from(octal(&header[124..136])?).map_err(|_| Error::Truncated)?;
        let padded = size
            .checked_next_multiple_of(BLOCK)
            .ok_or(Error::Truncated)?;
        let end = BLOCK.checked_add(padded).ok_or(Error::Truncated)?;
        let data = self.rest.get(BLOCK..BLOCK + size).ok_or(Error::Truncated)?;
        let kind = match header[156] {
            b'0' | 0 => Kind::File,
            b'5' => Kind::Directory,
            _ => Kind::Other,
        };
        let mut prefix = text(&header[345..500])?;
        let mut name = text(&header[..100])?;
        if prefix.is_empty() {
            name = name.strip_prefix("./").unwrap_or(name);
        } else {
            prefix = prefix.strip_prefix("./").unwrap_or(prefix);
        }
        if kind == Kind::Directory {
            name = name.strip_suffix('/').unwrap_or(name);
        }
        // The last member's padding may be cut short; the data is complete.
        self.rest = self.rest.get(end..).unwrap_or(&[]);
        Ok(Some(Entry {
            prefix,
            name,
            kind,
            data,
        }))
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = Result<Entry<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.parse_one() {
            Ok(Some(entry)) => Some(Ok(entry)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// The bytes of the regular file `path` in `archive`. Stops at the first
/// malformed header.
pub fn find<'a>(archive: &'a [u8], path: &str) -> Option<&'a [u8]> {
    entries(archive)
        .map_while(Result::ok)
        .find(|e| e.kind == Kind::File && e.is(path))
        .map(|e| e.data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Builds one ustar header plus data, padded.
    fn member(name: &str, prefix: &str, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut h = [0u8; BLOCK];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[108..115].copy_from_slice(b"0000000");
        h[116..123].copy_from_slice(b"0000000");
        let size = format!("{:011o}", data.len());
        h[124..135].copy_from_slice(size.as_bytes());
        h[136..147].copy_from_slice(b"00000000000");
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        let sum = format!("{sum:06o}\0 ");
        h[148..156].copy_from_slice(sum.as_bytes());
        let mut out = h.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(BLOCK), 0);
        out
    }

    fn archive(members: &[Vec<u8>]) -> Vec<u8> {
        let mut out: Vec<u8> = members.concat();
        out.extend_from_slice(&[0; 2 * BLOCK]);
        out
    }

    #[test]
    fn octal_fields() {
        assert_eq!(octal(b"00000000017\0"), Ok(15));
        assert_eq!(octal(b"  17 \0"), Ok(15));
        assert_eq!(octal(b"\0\0\0"), Ok(0));
        assert_eq!(octal(b"0009"), Err(Error::BadField));
        assert_eq!(octal(b"7777777777777777777777"), Err(Error::BadField));
    }

    #[test]
    fn reads_members() {
        let tar = archive(&[
            member("hello", "", b'0', b"hello world"),
            member("dir/", "", b'5', b""),
            member("./spin", "", 0, &[1; 600]),
            member("long", "some/dir", b'0', b"x"),
        ]);
        let all: Vec<_> = entries(&tar).collect::<Result<_, _>>().unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].name, "hello");
        assert_eq!(all[0].data, b"hello world");
        assert_eq!(all[1].kind, Kind::Directory);
        assert_eq!(all[1].name, "dir");
        assert_eq!(all[2].name, "spin");
        assert_eq!(all[2].data.len(), 600);
        assert!(all[3].is("some/dir/long"));
        assert!(!all[3].is("long"));
        assert_eq!(find(&tar, "hello"), Some(&b"hello world"[..]));
        assert_eq!(find(&tar, "spin").map(<[u8]>::len), Some(600));
        assert_eq!(find(&tar, "some/dir/long"), Some(&b"x"[..]));
        assert_eq!(find(&tar, "dir"), None);
        assert_eq!(find(&tar, "missing"), None);
    }

    #[test]
    fn empty_archives() {
        assert_eq!(entries(&[]).count(), 0);
        assert_eq!(entries(&[0; 1024]).count(), 0);
        assert_eq!(find(&[], "hello"), None);
    }

    #[test]
    fn rejects_damage() {
        let good = archive(&[member("a", "", b'0', b"abc")]);
        let mut bad = good.clone();
        bad[0] = b'b'; // name changed, checksum not
        assert_eq!(entries(&bad).next(), Some(Err(Error::BadChecksum)));
        let mut bad = good.clone();
        bad[257] = b'x';
        assert_eq!(entries(&bad).next(), Some(Err(Error::BadMagic)));
        // Data cut off.
        assert_eq!(
            entries(&good[..BLOCK + 2]).next(),
            Some(Err(Error::Truncated))
        );
        // Header cut off.
        assert_eq!(entries(&good[..100]).next(), Some(Err(Error::Truncated)));
        // Iteration stops after an error.
        let mut it = entries(&bad);
        assert!(it.next().is_some());
        assert_eq!(it.next(), None);
    }

    #[test]
    fn real_initrd_if_built() {
        // build/initrd.tar exists after `make initrd`.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../build/initrd.tar");
        let Ok(tar) = std::fs::read(path) else {
            return;
        };
        let names: Vec<_> = entries(&tar)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .iter()
            .map(|e| e.name.to_string())
            .collect();
        for program in ["hello", "spin", "fault"] {
            assert!(
                names.iter().any(|n| n == program),
                "{program} missing: {names:?}"
            );
            let data = find(&tar, program).unwrap();
            assert_eq!(&data[..4], b"\x7FELF");
        }
    }
}
