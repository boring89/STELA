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

/// The archive as a folder tree: every member path (`a/b/c`) becomes
/// folders `a`, `a/b` and the file `c`, whether or not the archive has
/// directory members. Node ids: 1 is the root, the others are 2, 3, ...
/// in the order the members appear.
#[derive(Debug)]
pub struct Tree<'a> {
    nodes: alloc::vec::Vec<TreeNode<'a>>,
}

/// One file or folder of a [`Tree`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode<'a> {
    /// Its id.
    pub id: u64,
    /// The folder it is in (the root is its own parent).
    pub parent: u64,
    /// The last component of its path.
    pub name: alloc::string::String,
    /// A folder.
    pub is_dir: bool,
    /// A file's bytes (empty for folders).
    pub data: &'a [u8],
}

/// The root folder's id.
pub const TREE_ROOT: u64 = 1;

impl<'a> Tree<'a> {
    /// Builds the tree of `archive`; stops at the first malformed header.
    /// A later member with the same path replaces an earlier file.
    pub fn new(archive: &'a [u8]) -> Tree<'a> {
        let mut tree = Tree {
            nodes: alloc::vec![TreeNode {
                id: TREE_ROOT,
                parent: TREE_ROOT,
                name: alloc::string::String::new(),
                is_dir: true,
                data: &[],
            }],
        };
        for e in entries(archive).map_while(Result::ok) {
            if e.kind == Kind::Other {
                continue;
            }
            let parts: alloc::vec::Vec<&str> = e
                .prefix
                .split('/')
                .chain(e.name.split('/'))
                .filter(|p| !p.is_empty() && *p != ".")
                .collect();
            let Some((last, folders)) = parts.split_last() else {
                continue;
            };
            let mut dir = TREE_ROOT;
            for folder in folders {
                dir = tree.folder(dir, folder);
            }
            if e.kind == Kind::Directory {
                tree.folder(dir, last);
            } else {
                tree.nodes
                    .retain(|n| !(n.parent == dir && n.name == *last && !n.is_dir));
                let id = tree.next_id();
                tree.nodes.push(TreeNode {
                    id,
                    parent: dir,
                    name: (*last).into(),
                    is_dir: false,
                    data: e.data,
                });
            }
        }
        tree
    }

    fn next_id(&self) -> u64 {
        self.nodes.iter().map(|n| n.id).max().unwrap_or(TREE_ROOT) + 1
    }

    /// The folder `name` in `dir`, made if missing.
    fn folder(&mut self, dir: u64, name: &str) -> u64 {
        if let Some(n) = self
            .nodes
            .iter()
            .find(|n| n.parent == dir && n.is_dir && n.name == name && n.id != TREE_ROOT)
        {
            return n.id;
        }
        let id = self.next_id();
        self.nodes.push(TreeNode {
            id,
            parent: dir,
            name: name.into(),
            is_dir: true,
            data: &[],
        });
        id
    }

    /// The node with id `id`.
    pub fn get(&self, id: u64) -> Option<&TreeNode<'a>> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The item `name` in folder `dir` (ASCII case ignored).
    pub fn lookup(&self, dir: u64, name: &str) -> Option<&TreeNode<'a>> {
        self.children(dir).find(|n| n.name == name).or_else(|| {
            self.children(dir)
                .find(|n| n.name.eq_ignore_ascii_case(name))
        })
    }

    /// The items of folder `dir`, in archive order.
    pub fn children(&self, dir: u64) -> impl Iterator<Item = &TreeNode<'a>> {
        self.nodes
            .iter()
            .filter(move |n| n.parent == dir && n.id != TREE_ROOT)
    }
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
    fn tree_of_paths() {
        let tar = archive(&[
            member("hello", "", b'0', b"hi"),
            member("linux/hello", "", b'0', b"elf"),
            member("apps/", "", b'5', b""),
            member("exec", "apps/Viewer.app", b'0', b"v"),
            member("hello", "", b'0', b"again"),
        ]);
        let t = Tree::new(&tar);
        let root: Vec<_> = t.children(TREE_ROOT).map(|n| n.name.as_str()).collect();
        assert_eq!(root, ["linux", "apps", "hello"]);
        let hello = t.lookup(TREE_ROOT, "HELLO").unwrap();
        assert_eq!(hello.data, b"again");
        let linux = t.lookup(TREE_ROOT, "linux").unwrap();
        assert!(linux.is_dir);
        assert_eq!(t.lookup(linux.id, "hello").unwrap().data, b"elf");
        let apps = t.lookup(TREE_ROOT, "apps").unwrap();
        let viewer = t.lookup(apps.id, "Viewer.app").unwrap();
        assert!(viewer.is_dir);
        let exec = t.lookup(viewer.id, "exec").unwrap();
        assert_eq!(t.get(exec.id).unwrap().parent, viewer.id);
        assert!(t.lookup(TREE_ROOT, "exec").is_none());
        assert!(t.get(TREE_ROOT).unwrap().is_dir);
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
