// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! FAT16 and FAT32, read only (Microsoft "FAT: General Overview of On-Disk
//! Format" 1.03): enough to read the EFI System Partition. Pure logic over
//! a [`BlockDevice`], tested on the host with images made by mtools.
//!
//! ```text
//!  sector 0     boot sector with the BPB: bytes per sector (11), sectors
//!               per cluster (13), reserved sectors (14), FAT count (16),
//!               root entries (17, FAT16), total sectors (19 / 32), FAT
//!               size (22 / 36 on FAT32), root cluster (44, FAT32)
//!  reserved .. FATs .. (FAT16: fixed root directory) .. data area
//!  cluster n starts at data area + (n - 2) * cluster size
//! ```
//!
//! The FAT type follows from the number of data clusters alone: fewer
//! than 4085 is FAT12 (not supported), fewer than 65525 FAT16, else FAT32.
//! Directory entries are 32 bytes; long names (attribute 0x0F) precede
//! their short entry in reverse order, 13 UTF-16 units each.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use block::{BLOCK_SIZE, BlockDevice, BlockError};

/// Why a FAT volume could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FatError {
    /// Not a FAT16/FAT32 boot sector.
    NotFat,
    /// FAT12 (too few clusters).
    Fat12,
    /// A cluster chain or entry points outside the volume, or loops.
    Corrupt,
    /// The device failed.
    Device(BlockError),
}

impl From<BlockError> for FatError {
    fn from(e: BlockError) -> FatError {
        FatError::Device(e)
    }
}

/// FAT16 or FAT32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FatKind {
    /// 16-bit FAT entries, fixed root directory.
    Fat16,
    /// 28-bit FAT entries, root directory in a cluster chain.
    Fat32,
}

/// A directory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FatEntry {
    /// The long name if there is one, else the short name.
    pub name: String,
    /// A directory.
    pub is_dir: bool,
    /// Size in bytes (0 for directories).
    pub size: u32,
    /// First cluster (0: empty file).
    pub cluster: u32,
    /// Last modification: (year, month, day, hour, minute, second).
    pub modified: (u16, u8, u8, u8, u8, u8),
}

/// Where a directory's entries are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// The FAT16 root directory region.
    FixedRoot,
    /// A cluster chain starting here.
    Chain(u32),
}

/// A mounted FAT volume.
pub struct FatVolume<D> {
    device: D,
    kind: FatKind,
    bytes_per_sector: u32,
    cluster_bytes: u32,
    fat_start: u64,
    root_start: u64,
    root_entries: u32,
    data_start: u64,
    clusters: u32,
    root_cluster: u32,
    label: String,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The short name of an entry: "NAME.EXT", lower-cased where the entry's
/// case flags (byte 12: 0x08 base, 0x10 extension, Windows NT) say so.
fn short_name(e: &[u8]) -> String {
    let flags = e[12];
    let part = |bytes: &[u8], lower: bool| -> String {
        let s: String = bytes
            .iter()
            .map(|&b| char::from(if b == 0x05 { 0xE5 } else { b }))
            .collect::<String>()
            .trim_end()
            .into();
        if lower { s.to_ascii_lowercase() } else { s }
    };
    let base = part(&e[..8], flags & 0x08 != 0);
    let ext = part(&e[8..11], flags & 0x10 != 0);
    if ext.is_empty() {
        base
    } else {
        alloc::format!("{base}.{ext}")
    }
}

/// The checksum of a short name that its long-name entries carry.
fn lfn_checksum(short: &[u8]) -> u8 {
    short[..11]
        .iter()
        .fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b))
}

impl<D: BlockDevice> FatVolume<D> {
    /// Reads the boot sector of `device` and checks it.
    pub fn mount(device: D) -> Result<FatVolume<D>, FatError> {
        let mut b = vec![0u8; BLOCK_SIZE];
        device.read(0, &mut b)?;
        if u16_at(&b, 510) != 0xAA55 || (b[0] != 0xEB && b[0] != 0xE9) {
            return Err(FatError::NotFat);
        }
        let bps = u32::from(u16_at(&b, 11));
        let spc = u32::from(b[13]);
        let reserved = u64::from(u16_at(&b, 14));
        let fats = u64::from(b[16]);
        let root_entries = u32::from(u16_at(&b, 17));
        let total = match u16_at(&b, 19) {
            0 => u64::from(u32_at(&b, 32)),
            n => u64::from(n),
        };
        let fat_size = match u16_at(&b, 22) {
            0 => u64::from(u32_at(&b, 36)),
            n => u64::from(n),
        };
        if ![512, 1024, 2048, 4096].contains(&bps)
            || spc == 0
            || !spc.is_power_of_two()
            || fats == 0
            || fat_size == 0
        {
            return Err(FatError::NotFat);
        }
        let bps64 = u64::from(bps);
        let root_sectors = (u64::from(root_entries) * 32).div_ceil(bps64);
        let data_sector = reserved + fats * fat_size + root_sectors;
        let clusters = total.saturating_sub(data_sector) / u64::from(spc);
        let kind = if clusters < 4085 {
            return Err(FatError::Fat12);
        } else if clusters < 65525 {
            FatKind::Fat16
        } else {
            FatKind::Fat32
        };
        if total * bps64 > device.block_count() * BLOCK_SIZE as u64 {
            return Err(FatError::Corrupt);
        }
        let label_at = if kind == FatKind::Fat32 { 71 } else { 43 };
        let label = String::from_utf8_lossy(&b[label_at..label_at + 11])
            .trim_end()
            .into();
        Ok(FatVolume {
            device,
            kind,
            bytes_per_sector: bps,
            cluster_bytes: bps * spc,
            fat_start: reserved * bps64,
            root_start: (reserved + fats * fat_size) * bps64,
            root_entries,
            data_start: data_sector * bps64,
            clusters: clusters as u32,
            root_cluster: u32_at(&b, 44),
            label,
        })
    }

    /// FAT16 or FAT32.
    pub fn kind(&self) -> FatKind {
        self.kind
    }

    /// The volume label of the boot sector ("NO NAME" if none was set).
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Size of the data area in bytes.
    pub fn size(&self) -> u64 {
        u64::from(self.clusters) * u64::from(self.cluster_bytes)
    }

    /// Bytes per cluster.
    pub fn cluster_bytes(&self) -> u32 {
        self.cluster_bytes
    }

    /// The root directory.
    pub fn root(&self) -> Dir {
        match self.kind {
            FatKind::Fat16 => Dir::FixedRoot,
            FatKind::Fat32 => Dir::Chain(self.root_cluster),
        }
    }

    /// Reads `buf.len()` bytes at byte `offset` of the volume.
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), FatError> {
        let bs = BLOCK_SIZE as u64;
        let first = offset / bs;
        let skip = (offset % bs) as usize;
        let blocks = (skip + buf.len()).div_ceil(BLOCK_SIZE);
        let mut tmp = vec![0u8; blocks * BLOCK_SIZE];
        self.device.read(first, &mut tmp)?;
        buf.copy_from_slice(&tmp[skip..skip + buf.len()]);
        Ok(())
    }

    /// The FAT entry of `cluster`: the next cluster, or `None` at the end
    /// of the chain.
    fn next(&self, cluster: u32) -> Result<Option<u32>, FatError> {
        if cluster < 2 || cluster >= self.clusters + 2 {
            return Err(FatError::Corrupt);
        }
        let (offset, width) = match self.kind {
            FatKind::Fat16 => (u64::from(cluster) * 2, 2),
            FatKind::Fat32 => (u64::from(cluster) * 4, 4),
        };
        let mut b = [0u8; 4];
        self.read_bytes(self.fat_start + offset, &mut b[..width])?;
        let value = match self.kind {
            FatKind::Fat16 => {
                let v = u32::from(u16_at(&b, 0));
                if v >= 0xFFF8 {
                    return Ok(None);
                }
                v
            }
            FatKind::Fat32 => {
                let v = u32_at(&b, 0) & 0x0FFF_FFFF;
                if v >= 0x0FFF_FFF8 {
                    return Ok(None);
                }
                v
            }
        };
        if value < 2 || value >= self.clusters + 2 {
            return Err(FatError::Corrupt);
        }
        Ok(Some(value))
    }

    /// The clusters of the chain starting at `first`.
    fn chain(&self, first: u32) -> Result<Vec<u32>, FatError> {
        let mut out = Vec::new();
        let mut at = Some(first);
        while let Some(c) = at {
            if out.len() > self.clusters as usize {
                return Err(FatError::Corrupt);
            }
            out.push(c);
            at = self.next(c)?;
        }
        Ok(out)
    }

    fn cluster_offset(&self, cluster: u32) -> u64 {
        self.data_start + u64::from(cluster - 2) * u64::from(self.cluster_bytes)
    }

    /// The raw bytes of a directory.
    fn dir_bytes(&self, dir: Dir) -> Result<Vec<u8>, FatError> {
        match dir {
            Dir::FixedRoot => {
                let mut b = vec![0u8; self.root_entries as usize * 32];
                self.read_bytes(self.root_start, &mut b)?;
                Ok(b)
            }
            Dir::Chain(first) => {
                let chain = self.chain(first)?;
                let cb = self.cluster_bytes as usize;
                let mut b = vec![0u8; chain.len() * cb];
                for (i, &c) in chain.iter().enumerate() {
                    self.read_bytes(self.cluster_offset(c), &mut b[i * cb..(i + 1) * cb])?;
                }
                Ok(b)
            }
        }
    }

    /// The entries of a directory (without "." and "..", the volume label
    /// and deleted entries).
    pub fn read_dir(&self, dir: Dir) -> Result<Vec<FatEntry>, FatError> {
        let bytes = self.dir_bytes(dir)?;
        let mut out = Vec::new();
        let mut long: Vec<(u8, [u16; 13])> = Vec::new();
        let mut long_sum = 0u8;
        for e in bytes.as_chunks::<32>().0 {
            match e[0] {
                0x00 => break,
                0xE5 => {
                    long.clear();
                    continue;
                }
                _ => {}
            }
            let attr = e[11];
            if attr & 0x3F == 0x0F {
                let mut units = [0u16; 13];
                let positions = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                for (u, &p) in units.iter_mut().zip(positions.iter()) {
                    *u = u16_at(e, p);
                }
                if e[0] & 0x40 != 0 {
                    long.clear();
                }
                long_sum = e[13];
                long.push((e[0] & 0x1F, units));
                continue;
            }
            if attr & 0x08 != 0 || e[0] == b'.' {
                long.clear();
                continue;
            }
            let name = if !long.is_empty() && long_sum == lfn_checksum(e) {
                long.sort_by_key(|(seq, _)| *seq);
                let units: Vec<u16> = long
                    .iter()
                    .flat_map(|(_, u)| u.iter().copied())
                    .take_while(|&u| u != 0 && u != 0xFFFF)
                    .collect();
                char::decode_utf16(units)
                    .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect()
            } else {
                short_name(e)
            };
            long.clear();
            let time = u16_at(e, 22);
            let date = u16_at(e, 24);
            out.push(FatEntry {
                name,
                is_dir: attr & 0x10 != 0,
                size: u32_at(e, 28),
                cluster: u32::from(u16_at(e, 20)) << 16 | u32::from(u16_at(e, 26)),
                modified: (
                    1980 + (date >> 9),
                    ((date >> 5) & 0xF) as u8,
                    (date & 0x1F) as u8,
                    (time >> 11) as u8,
                    ((time >> 5) & 0x3F) as u8,
                    ((time & 0x1F) * 2) as u8,
                ),
            });
        }
        Ok(out)
    }

    /// The entry called `name` in `dir`, compared without regard to ASCII
    /// case (FAT keeps case but ignores it).
    pub fn lookup(&self, dir: Dir, name: &str) -> Result<Option<FatEntry>, FatError> {
        Ok(self
            .read_dir(dir)?
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(name)))
    }

    /// The entry at `path` ("EFI/BOOT/x"; "" or "/" is not an entry).
    pub fn resolve(&self, path: &str) -> Result<Option<FatEntry>, FatError> {
        let mut dir = self.root();
        let mut found = None;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            if let Some(FatEntry { is_dir: false, .. }) = found {
                return Ok(None);
            }
            let Some(entry) = self.lookup(dir, part)? else {
                return Ok(None);
            };
            dir = self.dir_of(&entry);
            found = Some(entry);
        }
        Ok(found)
    }

    /// The directory an entry describes.
    pub fn dir_of(&self, entry: &FatEntry) -> Dir {
        if entry.cluster == 0 {
            // ".." of a first-level directory points at cluster 0: the root.
            self.root()
        } else {
            Dir::Chain(entry.cluster)
        }
    }

    /// Reads file bytes at `offset` into `buf`; returns how many (0 at or
    /// past the end).
    pub fn read_file(
        &self,
        entry: &FatEntry,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, FatError> {
        let size = u64::from(entry.size);
        if entry.is_dir || offset >= size || buf.is_empty() || entry.cluster == 0 {
            return Ok(0);
        }
        let len = (size - offset).min(buf.len() as u64) as usize;
        let cb = u64::from(self.cluster_bytes);
        let chain = self.chain(entry.cluster)?;
        let mut done = 0usize;
        while done < len {
            let at = offset + done as u64;
            let index = (at / cb) as usize;
            let cluster = *chain.get(index).ok_or(FatError::Corrupt)?;
            let within = at % cb;
            let piece = ((cb - within) as usize).min(len - done);
            self.read_bytes(
                self.cluster_offset(cluster) + within,
                &mut buf[done..done + piece],
            )?;
            done += piece;
        }
        Ok(len)
    }

    /// Bytes per sector of the volume.
    pub fn bytes_per_sector(&self) -> u32 {
        self.bytes_per_sector
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use block::mem::MemDisk;

    fn image() -> Option<MemDisk> {
        // build/images/fat-test.img comes from tools/make_test_images.py.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../build/images/fat-test.img");
        std::fs::read(path).ok().map(MemDisk::from_bytes)
    }

    fn pattern(size: usize) -> Vec<u8> {
        (0..size)
            .map(|i| ((i * 7 + i / 4096) % 251) as u8)
            .collect()
    }

    #[test]
    fn mtools_image_if_built() {
        let Some(disk) = image() else {
            return;
        };
        let fat = FatVolume::mount(&disk).unwrap();
        assert_eq!(fat.kind(), FatKind::Fat32);
        assert_eq!(fat.label(), "BOOT");
        let root = fat.read_dir(fat.root()).unwrap();
        let names: Vec<_> = root.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["HELLO.TXT", "A long file name.txt", "big.bin", "EFI"]
        );
        let hello = fat.lookup(fat.root(), "hello.txt").unwrap().unwrap();
        let mut buf = [0u8; 64];
        let n = fat.read_file(&hello, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello from FAT\n");
        assert_eq!(fat.read_file(&hello, 6, &mut buf[..4]).unwrap(), 4);
        assert_eq!(&buf[..4], b"from");
        assert_eq!(fat.read_file(&hello, 100, &mut buf).unwrap(), 0);
        let long = fat.resolve("/a LONG file name.TXT").unwrap().unwrap();
        assert_eq!(long.size, 16);
        // A multi-cluster file read at an odd offset across clusters.
        let big = fat.resolve("big.bin").unwrap().unwrap();
        let expected = pattern(300 * 1024);
        let mut data = vec![0u8; 300 * 1024];
        assert_eq!(fat.read_file(&big, 0, &mut data).unwrap(), data.len());
        assert!(data == expected);
        let mut part = vec![0u8; 5000];
        fat.read_file(&big, 70_001, &mut part).unwrap();
        assert!(part[..] == expected[70_001..75_001]);
        let nested = fat.resolve("EFI/BOOT/nested.txt").unwrap().unwrap();
        assert_eq!(nested.size, 15);
        assert!(fat.resolve("EFI/BOOT/missing").unwrap().is_none());
        assert!(fat.resolve("HELLO.TXT/x").unwrap().is_none());
        let efi = fat.resolve("efi").unwrap().unwrap();
        assert!(efi.is_dir);
        assert!(hello.modified.0 >= 2026);
    }

    #[test]
    fn not_fat() {
        let disk = MemDisk::new(4);
        assert!(matches!(FatVolume::mount(&disk), Err(FatError::NotFat)));
        assert_eq!(lfn_checksum(b"HELLO   TXT"), lfn_checksum(b"HELLO   TXT"));
        let mut e = [b' '; 32];
        e[..5].copy_from_slice(b"HELLO");
        e[8..11].copy_from_slice(b"TXT");
        e[12] = 0x18;
        assert_eq!(short_name(&e), "hello.txt");
    }
}
