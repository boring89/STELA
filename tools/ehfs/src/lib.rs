// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the eHFS host tools share: an image file as a [`BlockDevice`]
//! (optionally a partition inside it, `--offset` in 512-byte sectors),
//! path lookup and the current time.
//!
//! Safety rule: a target must be a regular file. Block devices and
//! everything else are refused, with no option to override: the tools
//! are for images under `build/`, never for real disks.

#![forbid(unsafe_code)]

use block::{BLOCK_SIZE, BlockDevice, BlockError, check_request};
use ehfs::{Entry, Error, ROOT, Volume};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// A disk image file (or a window of it).
pub struct ImageFile {
    file: Mutex<File>,
    /// Byte offset of block 0.
    offset: u64,
    blocks: u64,
    writable: bool,
}

impl ImageFile {
    /// Opens `path`, which must be a regular file. The volume starts at
    /// sector `offset_sectors` and has `sectors` sectors (`None`: to the
    /// end of the file).
    pub fn open(
        path: &str,
        writable: bool,
        offset_sectors: u64,
        sectors: Option<u64>,
    ) -> Result<ImageFile, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
        if !meta.is_file() {
            return Err(format!(
                "{path}: not a regular file; the eHFS tools only work on image files"
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(writable)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;
        let offset = offset_sectors * 512;
        let available = meta.len().saturating_sub(offset) / 512;
        let sectors = sectors.unwrap_or(available).min(available);
        Ok(ImageFile {
            file: Mutex::new(file),
            offset,
            blocks: sectors * 512 / BLOCK_SIZE as u64,
            writable,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        match self.file.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl BlockDevice for ImageFile {
    fn block_count(&self) -> u64 {
        self.blocks
    }

    fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check_request(self.blocks, block, buf.len())?;
        self.lock()
            .read_exact_at(buf, self.offset + block * BLOCK_SIZE as u64)
            .map_err(|_| BlockError::Io)
    }

    fn write(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        if !self.writable {
            return Err(BlockError::ReadOnly);
        }
        check_request(self.blocks, block, buf.len())?;
        self.lock()
            .write_all_at(buf, self.offset + block * BLOCK_SIZE as u64)
            .map_err(|_| BlockError::Io)
    }

    fn read_only(&self) -> bool {
        !self.writable
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.lock().sync_data().map_err(|_| BlockError::Io)
    }
}

/// Nanoseconds since 1970 (UTC).
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// A short description of an eHFS error.
pub fn describe(e: Error) -> &'static str {
    match e {
        Error::Io => "I/O error",
        Error::Corrupt => "the volume is damaged",
        Error::NoSpace => "no space left",
        Error::Invalid => "invalid argument",
        Error::ReadOnly => "read-only",
        Error::TxnTooLarge => "transaction too large",
        Error::NotFound => "no such file or folder",
        Error::Exists => "already exists",
        Error::NotDir => "not a folder",
        Error::IsDir => "is a folder",
        Error::NotEmpty => "folder not empty",
        Error::BadName => "not a valid name",
        Error::Unsupported => "unsupported eHFS feature",
    }
}

/// The parts of an eHFS path ("/a/b", "a/b").
pub fn parts(path: &str) -> Vec<&str> {
    path.split('/').filter(|p| !p.is_empty()).collect()
}

/// The item at `path` ("/" is the root).
pub fn resolve<D: BlockDevice>(v: &mut Volume<D>, path: &str) -> Result<Entry, Error> {
    let mut e = v.get(ROOT)?;
    for part in parts(path) {
        if !e.is_dir {
            return Err(Error::NotDir);
        }
        e = v.lookup(e.cnid, part.as_bytes())?;
    }
    Ok(e)
}

/// The folder and last name of `path`.
pub fn split_parent<D: BlockDevice>(v: &mut Volume<D>, path: &str) -> Result<(u64, String), Error> {
    let p = parts(path);
    let Some((last, dirs)) = p.split_last() else {
        return Err(Error::Invalid);
    };
    let mut dir = v.get(ROOT)?;
    for part in dirs {
        dir = v.lookup(dir.cnid, part.as_bytes())?;
        if !dir.is_dir {
            return Err(Error::NotDir);
        }
    }
    Ok((dir.cnid, (*last).to_string()))
}

/// Parses `--offset N` and `--sectors N` out of `args`; returns the rest.
pub fn take_geometry(args: Vec<String>) -> Result<(u64, Option<u64>, Vec<String>), String> {
    let mut offset = 0;
    let mut sectors = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--offset" | "--sectors" => {
                let v = it
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
                    .ok_or(format!("{a} needs a number"))?;
                if a == "--offset" {
                    offset = v;
                } else {
                    sectors = Some(v);
                }
            }
            "--device" => return Err(String::from("--device is not supported: images only")),
            _ => rest.push(a),
        }
    }
    Ok((offset, sectors, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_is_not_a_regular_file() {
        let e = ImageFile::open("/dev/null", false, 0, None).err().unwrap();
        assert!(e.contains("not a regular file"), "{e}");
        assert!(ImageFile::open("/", false, 0, None).is_err());
        assert!(ImageFile::open("/nonexistent/image", false, 0, None).is_err());
        assert!(take_geometry(vec!["--device".into()]).is_err());
    }

    #[test]
    fn image_round_trip() {
        let dir = std::env::temp_dir().join(format!("ehfs-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v.img");
        std::fs::write(&path, vec![0u8; 8 << 20]).unwrap();
        let p = path.to_str().unwrap();
        // A volume at sector 2048 (1 MiB) of the image.
        let img = ImageFile::open(p, true, 2048, None).unwrap();
        assert_eq!(img.block_count(), 7 * 256);
        ehfs::format(
            &img,
            &ehfs::FormatOptions {
                name: "Test",
                uuid: [1; 16],
                now: now(),
                journal_blocks: 0,
            },
        )
        .unwrap();
        let mut v = Volume::mount(&img, false).unwrap();
        let docs = v.create(ROOT, b"docs", true, now()).unwrap();
        let f = v.create(docs.cnid, b"x.txt", false, now()).unwrap();
        v.write(f.cnid, false, 0, b"hello", now()).unwrap();
        assert_eq!(resolve(&mut v, "/docs/X.TXT").unwrap().cnid, f.cnid);
        assert_eq!(
            split_parent(&mut v, "docs/new").unwrap(),
            (docs.cnid, "new".into())
        );
        assert!(v.check().clean());
        v.unmount().unwrap();
        // The bytes before the volume are untouched.
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes[..1 << 20].iter().all(|&b| b == 0));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
