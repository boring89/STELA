// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! GUID Partition Tables (UEFI Specification 2.10, 5.3).
//!
//! ```text
//!  LBA 0      protective MBR
//!  LBA 1      header: "EFI PART", revision, header size (92), header CRC32,
//!             my LBA, alternate LBA, first/last usable LBA, disk GUID,
//!             entry array LBA (72), entry count (80), entry size (84),
//!             entry array CRC32 (88)
//!  LBA 2..    entries (128 bytes): type GUID, unique GUID, first LBA (32),
//!             last LBA (40, inclusive), attributes, name (UTF-16LE, 72 bytes)
//! ```
//!
//! An LBA is a device sector: 512 bytes, or 4096 on 4Kn disks. The block
//! layer works in 4 KiB blocks, so the header is looked for at byte 512
//! (512-byte sectors) and at byte 4096 (4096-byte sectors).

use crate::crc32::crc32;
use crate::{BLOCK_SIZE, BlockDevice, BlockError};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A GUID as it is stored (the first three fields little endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// The all-zero GUID: an unused entry.
    pub const ZERO: Guid = Guid([0; 16]);
    /// EFI System Partition, C12A7328-F81F-11D2-BA4B-00A0C93EC93B.
    pub const ESP: Guid = Guid([
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ]);
}

impl core::fmt::Display for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-",
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u16::from_le_bytes([b[4], b[5]]),
            u16::from_le_bytes([b[6], b[7]]),
            b[8],
            b[9]
        )?;
        for x in &b[10..] {
            write!(f, "{x:02X}")?;
        }
        Ok(())
    }
}

/// One used partition entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GptPartition {
    /// Index in the entry array (0-based).
    pub index: u32,
    /// Partition type.
    pub kind: Guid,
    /// First LBA.
    pub first_lba: u64,
    /// Last LBA (inclusive).
    pub last_lba: u64,
    /// The name.
    pub name: String,
}

/// A parsed table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpt {
    /// Sector size the table uses (512 or 4096).
    pub sector_size: u32,
    /// The used entries.
    pub partitions: Vec<GptPartition>,
}

impl Gpt {
    /// Blocks (4 KiB) of `p`: (first block, count). `None` when the
    /// partition does not start on a 4 KiB boundary; a partial block at
    /// the end is left out (sgdisk ends the last partition where the
    /// backup table starts, which need not be aligned).
    pub fn blocks_of(&self, p: &GptPartition) -> Option<(u64, u64)> {
        let ss = u64::from(self.sector_size);
        let start = p.first_lba.checked_mul(ss)?;
        let end = p.last_lba.checked_add(1)?.checked_mul(ss)?;
        let bs = BLOCK_SIZE as u64;
        if !start.is_multiple_of(bs) || end / bs <= start / bs {
            return None;
        }
        Some((start / bs, end / bs - start / bs))
    }
}

/// Why a table was not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GptError {
    /// No "EFI PART" header.
    NotGpt,
    /// The header or entry array CRC does not match, or a field is out of
    /// range.
    Corrupt,
    /// The device failed.
    Device(BlockError),
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32_at(b, at)) | u64::from(u32_at(b, at + 4)) << 32
}

/// Checks a 512-byte (or longer) header sector; returns (entry array LBA,
/// count, entry size, array CRC).
fn parse_header(sector: &[u8]) -> Result<(u64, u32, u32, u32), GptError> {
    if !sector.starts_with(b"EFI PART") {
        return Err(GptError::NotGpt);
    }
    let size = u32_at(sector, 12) as usize;
    if !(92..=512).contains(&size) {
        return Err(GptError::Corrupt);
    }
    let mut header = sector[..size].to_vec();
    let stored = u32_at(&header, 16);
    header[16..20].fill(0);
    if crc32(&header) != stored {
        return Err(GptError::Corrupt);
    }
    let entry_size = u32_at(sector, 84);
    let count = u32_at(sector, 80);
    if entry_size < 128 || !entry_size.is_multiple_of(8) || count > 4096 {
        return Err(GptError::Corrupt);
    }
    Ok((u64_at(sector, 72), count, entry_size, u32_at(sector, 88)))
}

/// Reads the partition table of `device`.
pub fn read(device: &dyn BlockDevice) -> Result<Gpt, GptError> {
    let mut first = vec![0u8; BLOCK_SIZE];
    device.read(0, &mut first).map_err(GptError::Device)?;
    let (sector_size, header) = if first[512..].starts_with(b"EFI PART") {
        (512u32, first[512..1024].to_vec())
    } else {
        let mut second = vec![0u8; BLOCK_SIZE];
        if device.block_count() < 2 {
            return Err(GptError::NotGpt);
        }
        device.read(1, &mut second).map_err(GptError::Device)?;
        (4096, second)
    };
    let (array_lba, count, entry_size, array_crc) = parse_header(&header)?;
    let bytes = count as usize * entry_size as usize;
    let start = array_lba
        .checked_mul(u64::from(sector_size))
        .ok_or(GptError::Corrupt)?;
    let first_block = start / BLOCK_SIZE as u64;
    let skip = (start % BLOCK_SIZE as u64) as usize;
    let blocks = (skip + bytes).div_ceil(BLOCK_SIZE);
    if first_block + blocks as u64 > device.block_count() {
        return Err(GptError::Corrupt);
    }
    let mut array = vec![0u8; blocks * BLOCK_SIZE];
    device
        .read(first_block, &mut array)
        .map_err(GptError::Device)?;
    let array = &array[skip..skip + bytes];
    if crc32(array) != array_crc {
        return Err(GptError::Corrupt);
    }
    let mut partitions = Vec::new();
    for (index, entry) in array.chunks_exact(entry_size as usize).enumerate() {
        let mut kind = [0u8; 16];
        kind.copy_from_slice(&entry[..16]);
        if kind == [0; 16] {
            continue;
        }
        let units: Vec<u16> = entry[56..128]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&u| u != 0)
            .collect();
        let name = char::decode_utf16(units)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        let first_lba = u64_at(entry, 32);
        let last_lba = u64_at(entry, 40);
        if last_lba < first_lba {
            return Err(GptError::Corrupt);
        }
        partitions.push(GptPartition {
            index: index as u32,
            kind: Guid(kind),
            first_lba,
            last_lba,
            name,
        });
    }
    Ok(Gpt {
        sector_size,
        partitions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::MemDisk;

    fn utf16(name: &str) -> Vec<u8> {
        name.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    /// A GPT with two partitions on a disk of `blocks` blocks with
    /// `sector`-byte sectors.
    fn make(sector: usize, blocks: u64) -> MemDisk {
        let mut image = vec![0u8; blocks as usize * BLOCK_SIZE];
        let mut entries = vec![0u8; 128 * 128];
        let s = sector as u64;
        let mb = (1 << 20) / s;
        entries[..16].copy_from_slice(&Guid::ESP.0);
        entries[32..40].copy_from_slice(&mb.to_le_bytes());
        entries[40..48].copy_from_slice(&(2 * mb - 1).to_le_bytes());
        entries[56..56 + 20].copy_from_slice(&utf16("EFI system"));
        let e = &mut entries[128..256];
        e[..16].copy_from_slice(&[0xAF; 16]);
        e[32..40].copy_from_slice(&(2 * mb).to_le_bytes());
        e[40..48].copy_from_slice(&(4 * mb - 1).to_le_bytes());
        e[56..56 + 8].copy_from_slice(&utf16("Data"));
        let mut h = vec![0u8; 92];
        h[..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&1u64.to_le_bytes());
        h[72..80].copy_from_slice(&2u64.to_le_bytes());
        h[80..84].copy_from_slice(&128u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&crc32(&entries).to_le_bytes());
        let c = crc32(&h);
        h[16..20].copy_from_slice(&c.to_le_bytes());
        image[sector..sector + 92].copy_from_slice(&h);
        image[2 * sector..2 * sector + entries.len()].copy_from_slice(&entries);
        MemDisk::from_bytes(image)
    }

    #[test]
    fn tables_with_both_sector_sizes() {
        for sector in [512, 4096] {
            let disk = make(sector, 1024);
            let gpt = read(&disk).unwrap();
            assert_eq!(gpt.sector_size, sector as u32);
            assert_eq!(gpt.partitions.len(), 2);
            let esp = &gpt.partitions[0];
            assert_eq!(esp.kind, Guid::ESP);
            assert_eq!(esp.name, "EFI system");
            assert_eq!(gpt.blocks_of(esp), Some((256, 256)));
            assert_eq!(gpt.partitions[1].name, "Data");
            assert_eq!(gpt.partitions[1].index, 1);
            assert_eq!(gpt.blocks_of(&gpt.partitions[1]), Some((512, 512)));
        }
        assert_eq!(
            Guid::ESP.to_string(),
            "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
        );
    }

    #[test]
    fn errors() {
        assert_eq!(read(&MemDisk::new(4)), Err(GptError::NotGpt));
        let disk = make(512, 1024);
        let mut bytes = disk.to_bytes();
        bytes[512 + 40] ^= 1; // header CRC no longer matches
        assert_eq!(read(&MemDisk::from_bytes(bytes)), Err(GptError::Corrupt));
        let mut bytes = disk.to_bytes();
        bytes[1024 + 33] ^= 1; // entry array CRC
        assert_eq!(read(&MemDisk::from_bytes(bytes)), Err(GptError::Corrupt));
        // Not 4 KiB aligned.
        let p = GptPartition {
            index: 0,
            kind: Guid::ESP,
            first_lba: 34,
            last_lba: 2047,
            name: String::new(),
        };
        let gpt = Gpt {
            sector_size: 512,
            partitions: vec![],
        };
        assert_eq!(gpt.blocks_of(&p), None);
    }

    #[test]
    fn sgdisk_image_if_built() {
        // build/images/gpt-test.img comes from tools/make_test_images.py.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../build/images/gpt-test.img");
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let gpt = read(&MemDisk::from_bytes(bytes)).unwrap();
        assert_eq!(gpt.sector_size, 512);
        assert_eq!(gpt.partitions.len(), 2);
        assert_eq!(gpt.partitions[0].kind, Guid::ESP);
        assert_eq!(gpt.partitions[0].first_lba, 2048);
        assert_eq!(gpt.partitions[1].name, "Data");
        // 34816..=131038 sectors: the last partial block is left out.
        assert_eq!(gpt.blocks_of(&gpt.partitions[1]), Some((4352, 12027)));
    }
}
