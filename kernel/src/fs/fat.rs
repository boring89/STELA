// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! FAT16 and FAT32 (Microsoft "FAT: General Overview of On-Disk Format"
//! 1.03): reading (the EFI System Partition) and, P11 K42, writing (USB
//! sticks): files and folders are made, written, cut, renamed and removed;
//! new names get a short `NAME~N.EXT` entry and long-name entries; free
//! clusters are found by scanning the FAT, every FAT copy is written, and
//! FSInfo's free count is set to "unknown". Pure logic over a
//! [`BlockDevice`], tested on the host with images made by mtools.
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
    /// A name that is taken (writing).
    Exists,
    /// No such entry (writing).
    NotFound,
    /// A folder with entries in it cannot be removed.
    NotEmpty,
    /// No free cluster, or the FAT16 root directory is full.
    NoSpace,
    /// A name FAT cannot hold.
    BadName,
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
    /// Where its entries are: the folder, the byte offset of the short
    /// entry in it, and how many long-name entries come before it.
    pub slot: Option<(Dir, u32, u8)>,
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
    fats: u32,
    fat_bytes: u64,
    /// Where FSInfo is (FAT32), in bytes.
    fsinfo: Option<u64>,
    /// Where to look for a free cluster next.
    next_free: u32,
    /// FSInfo was marked "free count unknown".
    fsinfo_done: bool,
    /// Free clusters, once counted.
    free: Option<u32>,
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
            fats: fats as u32,
            fat_bytes: fat_size * bps64,
            fsinfo: (kind == FatKind::Fat32 && u16_at(&b, 48) != 0)
                .then(|| u64::from(u16_at(&b, 48)) * bps64),
            next_free: 2,
            fsinfo_done: false,
            free: None,
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
        for (index, e) in bytes.as_chunks::<32>().0.iter().enumerate() {
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
            let lfn = if !long.is_empty() && long_sum == lfn_checksum(e) {
                long.len() as u8
            } else {
                0
            };
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
                slot: Some((dir, (index * 32) as u32, lfn)),
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

    // ---- writing (P11 K42) ----

    /// Writes `data` at byte `offset` of the volume (whole blocks read,
    /// changed and written back).
    fn write_bytes(&mut self, offset: u64, data: &[u8]) -> Result<(), FatError> {
        let bs = BLOCK_SIZE as u64;
        let first = offset / bs;
        let skip = (offset % bs) as usize;
        let blocks = (skip + data.len()).div_ceil(BLOCK_SIZE);
        let mut tmp = vec![0u8; blocks * BLOCK_SIZE];
        self.device.read(first, &mut tmp)?;
        tmp[skip..skip + data.len()].copy_from_slice(data);
        self.device.write(first, &tmp)?;
        Ok(())
    }

    /// Sets FAT entry `cluster` in every copy of the FAT (FAT32 keeps the
    /// top four bits). The first change marks FSInfo's counts unknown.
    fn set_fat(&mut self, cluster: u32, value: u32) -> Result<(), FatError> {
        if !self.fsinfo_done {
            self.fsinfo_done = true;
            if let Some(at) = self.fsinfo {
                // Free count and next free (offsets 488, 492): unknown.
                self.write_bytes(at + 488, &[0xFF; 8])?;
            }
        }
        for copy in 0..u64::from(self.fats) {
            let base = self.fat_start + copy * self.fat_bytes;
            match self.kind {
                FatKind::Fat16 => {
                    self.write_bytes(base + u64::from(cluster) * 2, &(value as u16).to_le_bytes())?;
                }
                FatKind::Fat32 => {
                    let at = base + u64::from(cluster) * 4;
                    let mut old = [0u8; 4];
                    self.read_bytes(at, &mut old)?;
                    let v = (u32_at(&old, 0) & 0xF000_0000) | (value & 0x0FFF_FFFF);
                    self.write_bytes(at, &v.to_le_bytes())?;
                }
            }
        }
        Ok(())
    }

    fn end_of_chain(&self) -> u32 {
        match self.kind {
            FatKind::Fat16 => 0xFFFF,
            FatKind::Fat32 => 0x0FFF_FFFF,
        }
    }

    /// The raw FAT entry of `cluster` (0: free).
    fn fat_raw(&self, cluster: u32) -> Result<u32, FatError> {
        let mut b = [0u8; 4];
        Ok(match self.kind {
            FatKind::Fat16 => {
                self.read_bytes(self.fat_start + u64::from(cluster) * 2, &mut b[..2])?;
                u32::from(u16_at(&b, 0))
            }
            FatKind::Fat32 => {
                self.read_bytes(self.fat_start + u64::from(cluster) * 4, &mut b)?;
                u32_at(&b, 0) & 0x0FFF_FFFF
            }
        })
    }

    /// A free cluster, zeroed and marked as the end of a chain; linked
    /// after `prev` when given.
    fn alloc_cluster(&mut self, prev: Option<u32>) -> Result<u32, FatError> {
        let total = self.clusters;
        let start = self.next_free.clamp(2, total + 1);
        let mut found = None;
        for i in 0..total {
            let c = 2 + (start - 2 + i) % total;
            if self.fat_raw(c)? == 0 {
                found = Some(c);
                break;
            }
        }
        let c = found.ok_or(FatError::NoSpace)?;
        self.next_free = c + 1;
        if let Some(f) = self.free.as_mut() {
            *f = f.saturating_sub(1);
        }
        let eoc = self.end_of_chain();
        self.set_fat(c, eoc)?;
        let zero = vec![0u8; self.cluster_bytes as usize];
        self.write_bytes(self.cluster_offset(c), &zero)?;
        if let Some(p) = prev {
            self.set_fat(p, c)?;
        }
        Ok(c)
    }

    /// Frees the chain starting at `first` (0: nothing).
    fn free_chain(&mut self, first: u32) -> Result<(), FatError> {
        if first < 2 {
            return Ok(());
        }
        for c in self.chain(first)? {
            self.set_fat(c, 0)?;
            if let Some(f) = self.free.as_mut() {
                *f += 1;
            }
        }
        self.next_free = self.next_free.min(first);
        Ok(())
    }

    /// The volume byte offset of byte `offset` of directory `dir`.
    fn dir_offset(&self, dir: Dir, offset: u32) -> Result<u64, FatError> {
        match dir {
            Dir::FixedRoot => Ok(self.root_start + u64::from(offset)),
            Dir::Chain(first) => {
                let chain = self.chain(first)?;
                let cb = self.cluster_bytes;
                let c = *chain.get((offset / cb) as usize).ok_or(FatError::Corrupt)?;
                Ok(self.cluster_offset(c) + u64::from(offset % cb))
            }
        }
    }

    /// The offset of `n` free entries in a row in `dir` (the directory
    /// grows by a cluster when it has none; the FAT16 root cannot).
    fn free_slots(&mut self, dir: Dir, n: usize) -> Result<u32, FatError> {
        let bytes = self.dir_bytes(dir)?;
        let mut run = 0;
        for (i, e) in bytes.as_chunks::<32>().0.iter().enumerate() {
            if e[0] == 0 || e[0] == 0xE5 {
                run += 1;
                if run == n {
                    return Ok(((i + 1 - n) * 32) as u32);
                }
            } else {
                run = 0;
            }
        }
        let Dir::Chain(first) = dir else {
            return Err(FatError::NoSpace);
        };
        let last = *self.chain(first)?.last().ok_or(FatError::Corrupt)?;
        self.alloc_cluster(Some(last))?;
        // The free run may start in the old last cluster.
        Ok(((bytes.len() / 32 - run) * 32) as u32)
    }

    /// A short name (11 bytes) for `name` that no entry of `dir` has:
    /// `NAME~N.EXT`, or the name itself when it is a plain 8.3 name.
    fn short_for(&self, dir: Dir, name: &str) -> Result<([u8; 11], bool), FatError> {
        let taken: Vec<[u8; 11]> = self
            .dir_bytes(dir)?
            .as_chunks::<32>()
            .0
            .iter()
            .take_while(|e| e[0] != 0)
            .filter(|e| e[0] != 0xE5 && e[11] & 0x3F != 0x0F)
            .map(|e| {
                let mut s = [0u8; 11];
                s.copy_from_slice(&e[..11]);
                s
            })
            .collect();
        let (base, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
            _ => (name, ""),
        };
        let clean = |s: &str| -> Vec<u8> {
            s.chars()
                .filter(|c| *c != ' ' && *c != '.')
                .map(|c| {
                    let c = c.to_ascii_uppercase();
                    if c.is_ascii_alphanumeric() || "$%'-_@~`!(){}^#&".contains(c) {
                        c as u8
                    } else {
                        b'_'
                    }
                })
                .collect()
        };
        let (b, e) = (clean(base), clean(ext));
        let fits = !b.is_empty()
            && b.len() <= 8
            && e.len() <= 3
            && name.is_ascii()
            && name == name.to_ascii_uppercase()
            && b.len() == base.len()
            && e.len() == ext.len();
        let mut short = [b' '; 11];
        short[8..8 + e.len().min(3)].copy_from_slice(&e[..e.len().min(3)]);
        if fits {
            short[..b.len()].copy_from_slice(&b);
            if !taken.contains(&short) {
                return Ok((short, false));
            }
        }
        for n in 1..1_000_000u32 {
            let tail = alloc::format!("~{n}");
            let keep = (8 - tail.len()).min(b.len());
            let mut s = short;
            s[..8].fill(b' ');
            s[..keep].copy_from_slice(&b[..keep]);
            s[keep..keep + tail.len()].copy_from_slice(tail.as_bytes());
            if !taken.contains(&s) {
                return Ok((s, true));
            }
        }
        Err(FatError::NoSpace)
    }

    /// Writes entry bytes for `name` (long-name entries when `long`, then
    /// the short entry `short`) at `offset` of `dir`.
    fn write_entries(
        &mut self,
        dir: Dir,
        offset: u32,
        name: &str,
        long: bool,
        short: &[u8; 32],
    ) -> Result<u8, FatError> {
        let mut entries: Vec<[u8; 32]> = Vec::new();
        if long {
            let units: Vec<u16> = name.encode_utf16().collect();
            let count = units.len().div_ceil(13);
            let sum = lfn_checksum(&short[..11]);
            for seq in (1..=count).rev() {
                let mut e = [0u8; 32];
                e[0] = seq as u8 | if seq == count { 0x40 } else { 0 };
                e[11] = 0x0F;
                e[13] = sum;
                let positions = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                for (k, &p) in positions.iter().enumerate() {
                    let i = (seq - 1) * 13 + k;
                    let u = match i.cmp(&units.len()) {
                        core::cmp::Ordering::Less => units[i],
                        core::cmp::Ordering::Equal => 0,
                        core::cmp::Ordering::Greater => 0xFFFF,
                    };
                    e[p..p + 2].copy_from_slice(&u.to_le_bytes());
                }
                entries.push(e);
            }
        }
        entries.push(*short);
        for (i, e) in entries.iter().enumerate() {
            let at = self.dir_offset(dir, offset + 32 * i as u32)?;
            self.write_bytes(at, e)?;
        }
        Ok((entries.len() - 1) as u8)
    }

    /// Makes file or folder `name` in `dir` at time `now` (FAT date and
    /// time words).
    pub fn create(
        &mut self,
        dir: Dir,
        name: &str,
        is_dir: bool,
        now: (u16, u16),
    ) -> Result<FatEntry, FatError> {
        if name.is_empty()
            || name.len() > 255
            || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
            || name == "."
            || name == ".."
        {
            return Err(FatError::BadName);
        }
        if self.lookup(dir, name)?.is_some() {
            return Err(FatError::Exists);
        }
        let (short, long) = self.short_for(dir, name)?;
        let lfn = if long {
            name.encode_utf16().count().div_ceil(13)
        } else {
            0
        };
        let offset = self.free_slots(dir, lfn + 1)?;
        let cluster = if is_dir { self.alloc_cluster(None)? } else { 0 };
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(&short);
        e[11] = if is_dir { 0x10 } else { 0x20 };
        e[14..16].copy_from_slice(&now.1.to_le_bytes());
        e[16..18].copy_from_slice(&now.0.to_le_bytes());
        e[18..20].copy_from_slice(&now.0.to_le_bytes());
        e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        e[22..24].copy_from_slice(&now.1.to_le_bytes());
        e[24..26].copy_from_slice(&now.0.to_le_bytes());
        e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        let lfn = self.write_entries(dir, offset, name, long, &e)?;
        if is_dir {
            // "." and ".." (cluster 0 for the root).
            let parent = match dir {
                Dir::FixedRoot => 0,
                Dir::Chain(c) if c == self.root_cluster && self.kind == FatKind::Fat32 => 0,
                Dir::Chain(c) => c,
            };
            for (i, (dots, c)) in [(*b".          ", cluster), (*b"..         ", parent)]
                .iter()
                .enumerate()
            {
                let mut d = e;
                d[..11].copy_from_slice(dots);
                d[11] = 0x10;
                d[20..22].copy_from_slice(&((c >> 16) as u16).to_le_bytes());
                d[26..28].copy_from_slice(&(*c as u16).to_le_bytes());
                self.write_bytes(self.cluster_offset(cluster) + 32 * i as u64, &d)?;
            }
        }
        let mut entry = self.lookup(dir, name)?.ok_or(FatError::Corrupt)?;
        if entry.slot.is_none() {
            entry.slot = Some((dir, offset + 32 * u32::from(lfn), lfn));
        }
        Ok(entry)
    }

    /// Writes `entry`'s size and first cluster (and the time) back to its
    /// directory entry.
    fn store(&mut self, entry: &FatEntry, now: (u16, u16)) -> Result<(), FatError> {
        let (dir, offset, _) = entry.slot.ok_or(FatError::Corrupt)?;
        let at = self.dir_offset(dir, offset)?;
        let mut e = [0u8; 32];
        self.read_bytes(at, &mut e)?;
        e[18..20].copy_from_slice(&now.0.to_le_bytes());
        e[20..22].copy_from_slice(&((entry.cluster >> 16) as u16).to_le_bytes());
        e[22..24].copy_from_slice(&now.1.to_le_bytes());
        e[24..26].copy_from_slice(&now.0.to_le_bytes());
        e[26..28].copy_from_slice(&(entry.cluster as u16).to_le_bytes());
        e[28..32].copy_from_slice(&entry.size.to_le_bytes());
        self.write_bytes(at, &e)
    }

    /// Writes `data` at `offset` of file `entry` (growing it; gaps are
    /// zeros) and updates `entry`.
    pub fn write_file(
        &mut self,
        entry: &mut FatEntry,
        offset: u64,
        data: &[u8],
        now: (u16, u16),
    ) -> Result<(), FatError> {
        if entry.is_dir {
            return Err(FatError::BadName);
        }
        let end = offset + data.len() as u64;
        if end > u64::from(u32::MAX) {
            return Err(FatError::NoSpace);
        }
        let cb = u64::from(self.cluster_bytes);
        let mut chain = if entry.cluster >= 2 {
            self.chain(entry.cluster)?
        } else {
            Vec::new()
        };
        while (chain.len() as u64) * cb < end {
            let c = self.alloc_cluster(chain.last().copied())?;
            if chain.is_empty() {
                entry.cluster = c;
            }
            chain.push(c);
        }
        let mut done = 0usize;
        while done < data.len() {
            let at = offset + done as u64;
            let c = chain[(at / cb) as usize];
            let within = at % cb;
            let piece = ((cb - within) as usize).min(data.len() - done);
            self.write_bytes(self.cluster_offset(c) + within, &data[done..done + piece])?;
            done += piece;
        }
        if end > u64::from(entry.size) {
            entry.size = end as u32;
        }
        self.store(entry, now)
    }

    /// Cuts or extends file `entry` to `size` bytes.
    pub fn set_size(
        &mut self,
        entry: &mut FatEntry,
        size: u64,
        now: (u16, u16),
    ) -> Result<(), FatError> {
        if size > u64::from(entry.size) {
            let zeros = vec![0u8; (size - u64::from(entry.size)) as usize];
            let at = u64::from(entry.size);
            return self.write_file(entry, at, &zeros, now);
        }
        let cb = u64::from(self.cluster_bytes);
        let keep = size.div_ceil(cb) as usize;
        if entry.cluster >= 2 {
            let chain = self.chain(entry.cluster)?;
            if keep == 0 {
                self.free_chain(entry.cluster)?;
                entry.cluster = 0;
            } else if keep < chain.len() {
                let eoc = self.end_of_chain();
                self.set_fat(chain[keep - 1], eoc)?;
                self.free_chain(chain[keep])?;
            }
        }
        entry.size = size as u32;
        self.store(entry, now)
    }

    /// Marks `entry`'s directory entries deleted (its clusters stay).
    fn unlink(&mut self, entry: &FatEntry) -> Result<(), FatError> {
        let (dir, offset, lfn) = entry.slot.ok_or(FatError::Corrupt)?;
        for i in 0..=u32::from(lfn) {
            let at = self.dir_offset(dir, offset - 32 * i)?;
            self.write_bytes(at, &[0xE5])?;
        }
        Ok(())
    }

    /// Removes file or empty folder `name` from `dir`.
    pub fn remove(&mut self, dir: Dir, name: &str) -> Result<(), FatError> {
        let entry = self.lookup(dir, name)?.ok_or(FatError::NotFound)?;
        if entry.is_dir && !self.read_dir(self.dir_of(&entry))?.is_empty() {
            return Err(FatError::NotEmpty);
        }
        self.unlink(&entry)?;
        self.free_chain(entry.cluster)
    }

    /// Moves `name` of `dir` to `new_name` in `new_dir`.
    pub fn rename(
        &mut self,
        dir: Dir,
        name: &str,
        new_dir: Dir,
        new_name: &str,
        now: (u16, u16),
    ) -> Result<(), FatError> {
        let old = self.lookup(dir, name)?.ok_or(FatError::NotFound)?;
        if let Some(existing) = self.lookup(new_dir, new_name)?
            && existing.slot != old.slot
        {
            return Err(FatError::Exists);
        }
        // The new entries first (a file with no clusters), then point them
        // at the old data and drop the old entries.
        self.unlink(&old)?;
        let mut new = match self.create(new_dir, new_name, false, now) {
            Ok(e) => e,
            Err(e) => {
                // Put the old short entry back (its first byte).
                if let Some((d, off, _)) = old.slot {
                    let mut short = [0u8; 32];
                    let at = self.dir_offset(d, off)?;
                    self.read_bytes(at, &mut short)?;
                    let first = self.short_first_byte(&old);
                    short[0] = first;
                    self.write_bytes(at, &short)?;
                }
                return Err(e);
            }
        };
        new.cluster = old.cluster;
        new.size = old.size;
        new.is_dir = old.is_dir;
        self.store(&new, now)?;
        if old.is_dir {
            let (d, off, _) = new.slot.ok_or(FatError::Corrupt)?;
            let at = self.dir_offset(d, off)?;
            self.write_bytes(at + 11, &[0x10])?;
            // ".." of the moved folder.
            let parent = match new_dir {
                Dir::FixedRoot => 0,
                Dir::Chain(c) if c == self.root_cluster && self.kind == FatKind::Fat32 => 0,
                Dir::Chain(c) => c,
            };
            if old.cluster >= 2 {
                let dotdot = self.cluster_offset(old.cluster) + 32;
                self.write_bytes(dotdot + 20, &((parent >> 16) as u16).to_le_bytes())?;
                self.write_bytes(dotdot + 26, &(parent as u16).to_le_bytes())?;
            }
        }
        Ok(())
    }

    /// The first byte of `entry`'s short name as it was (for undoing an
    /// unlink): the first character of its short form.
    fn short_first_byte(&self, entry: &FatEntry) -> u8 {
        entry
            .name
            .bytes()
            .next()
            .map_or(b'_', |b| b.to_ascii_uppercase())
    }

    /// Free bytes (the FAT is counted the first time).
    pub fn free_bytes(&mut self) -> Result<u64, FatError> {
        let free = match self.free {
            Some(f) => f,
            None => {
                let mut n = 0u32;
                for c in 2..self.clusters + 2 {
                    if self.fat_raw(c)? == 0 {
                        n += 1;
                    }
                }
                self.free = Some(n);
                n
            }
        };
        Ok(u64::from(free) * u64::from(self.cluster_bytes))
    }
}

/// FAT's date and time words for nanoseconds since 1970 (UTC; FAT keeps
/// no zone): date = (year - 1980) << 9 | month << 5 | day, time = hour
/// << 11 | minute << 5 | second / 2.
pub fn fat_now(unix_ns: u64) -> (u16, u16) {
    let secs = (unix_ns / 1_000_000_000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = (yoe + era * 400 + i64::from(m <= 2)).clamp(1980, 2107);
    let date = ((y - 1980) << 9 | m << 5 | d) as u16;
    let time = ((rem / 3600) << 11 | (rem / 60 % 60) << 5 | ((rem % 60) / 2)) as u16;
    (date, time)
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
    fn writing_if_built() {
        let Some(disk) = image() else {
            return;
        };
        let now = fat_now(1_792_570_385 * 1_000_000_000);
        let mut fat = FatVolume::mount(&disk).unwrap();
        let free = fat.free_bytes().unwrap();
        let root = fat.root();
        // A long name with a short one made up, written across clusters.
        let mut f = fat
            .create(root, "Notes from STELA.txt", false, now)
            .unwrap();
        let data = pattern(10_000);
        fat.write_file(&mut f, 0, &data, now).unwrap();
        fat.write_file(&mut f, 9_000, b"tail", now).unwrap();
        let back = fat.lookup(root, "notes from stela.txt").unwrap().unwrap();
        assert_eq!(back.size, 10_000);
        let mut buf = vec![0u8; 10_000];
        fat.read_file(&back, 0, &mut buf).unwrap();
        assert_eq!(&buf[9_000..9_004], b"tail");
        assert_eq!(&buf[..100], &data[..100]);
        assert!(fat.free_bytes().unwrap() < free);
        assert_eq!(
            fat.create(root, "NOTES FROM STELA.TXT", false, now).err(),
            Some(FatError::Exists)
        );
        // A plain 8.3 name needs no long entries.
        let plain = fat.create(root, "PLAIN.TXT", false, now).unwrap();
        assert_eq!(plain.slot.map(|s| s.2), Some(0));
        // A folder, a file in it, a rename into it, removal.
        let d = fat.create(root, "new folder", true, now).unwrap();
        let dir = fat.dir_of(&d);
        let mut inner = fat.create(dir, "inner.txt", false, now).unwrap();
        fat.write_file(&mut inner, 0, b"in the folder", now)
            .unwrap();
        fat.rename(root, "PLAIN.TXT", dir, "moved.txt", now)
            .unwrap();
        assert!(fat.lookup(root, "PLAIN.TXT").unwrap().is_none());
        let names: Vec<String> = fat
            .read_dir(dir)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["inner.txt", "moved.txt"]);
        assert_eq!(
            fat.remove(root, "new folder").err(),
            Some(FatError::NotEmpty)
        );
        fat.remove(dir, "inner.txt").unwrap();
        fat.remove(dir, "moved.txt").unwrap();
        fat.remove(root, "new folder").unwrap();
        // Cut the file, then remove it: the space comes back.
        let mut f = fat.lookup(root, "Notes from STELA.txt").unwrap().unwrap();
        fat.set_size(&mut f, 5, now).unwrap();
        assert_eq!(
            fat.lookup(root, "Notes from STELA.txt")
                .unwrap()
                .unwrap()
                .size,
            5
        );
        fat.remove(root, "Notes from STELA.txt").unwrap();
        assert_eq!(fat.free_bytes().unwrap(), free);
        // The original files are still there and intact.
        let hello = fat.lookup(root, "hello.txt").unwrap().unwrap();
        let mut b = [0u8; 15];
        fat.read_file(&hello, 0, &mut b).unwrap();
        assert_eq!(&b, b"hello from FAT\n");
    }

    #[test]
    fn times() {
        let (date, time) = fat_now(1_792_570_385 * 1_000_000_000);
        // 2026-10-21 08:13:05: year 46, month 10, day 21; seconds / 2.
        assert_eq!(date, 46 << 9 | 10 << 5 | 21);
        assert_eq!(time, 8 << 11 | 13 << 5 | 2);
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
