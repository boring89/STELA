// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The MBR partition table (P11 K41: USB sticks usually have one): four
//! 16-byte entries at byte 446 of sector 0, then the signature 55 AA.
//! A table whose only use is to protect a GPT (type EE) is not one.

use alloc::vec::Vec;

/// One used entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MbrPartition {
    /// Its slot (0–3).
    pub index: u8,
    /// The type byte (0B/0C FAT32, 06/0E FAT16, 07 NTFS/exFAT, 83 Linux...).
    pub kind: u8,
    /// First sector (512 bytes).
    pub first_lba: u64,
    /// Sectors.
    pub sectors: u64,
}

/// The partitions of the MBR in `sector0` (the first 512 bytes of the
/// disk); `None` without the signature or with a protective GPT entry.
pub fn read(sector0: &[u8]) -> Option<Vec<MbrPartition>> {
    if sector0.len() < 512 || sector0[510] != 0x55 || sector0[511] != 0xAA {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..4 {
        let e = &sector0[446 + 16 * i..446 + 16 * (i + 1)];
        let kind = e[4];
        if kind == 0xEE {
            return None;
        }
        let first = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
        let sectors = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);
        // Status must be 00 or 80 (a boot sector of a FAT volume without a
        // table has other bytes there).
        if kind == 0 || sectors == 0 || (e[0] != 0 && e[0] != 0x80) {
            continue;
        }
        out.push(MbrPartition {
            index: i as u8,
            kind,
            first_lba: u64::from(first),
            sectors: u64::from(sectors),
        });
    }
    (!out.is_empty()).then_some(out)
}

impl MbrPartition {
    /// Its 4 KiB blocks (first, count) when it starts on a 4 KiB boundary.
    pub fn blocks(&self) -> Option<(u64, u64)> {
        self.first_lba
            .is_multiple_of(8)
            .then_some((self.first_lba / 8, self.sectors / 8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sector(entries: &[(u8, u8, u32, u32)]) -> Vec<u8> {
        let mut s = vec![0u8; 512];
        for (i, &(status, kind, first, count)) in entries.iter().enumerate() {
            let e = &mut s[446 + 16 * i..446 + 16 * (i + 1)];
            e[0] = status;
            e[4] = kind;
            e[8..12].copy_from_slice(&first.to_le_bytes());
            e[12..16].copy_from_slice(&count.to_le_bytes());
        }
        s[510] = 0x55;
        s[511] = 0xAA;
        s
    }

    #[test]
    fn tables() {
        let s = sector(&[(0x80, 0x0C, 2048, 30720), (0, 0, 0, 0), (0, 0x83, 40960, 8)]);
        let p = read(&s).expect("a table");
        assert_eq!(p.len(), 2);
        assert_eq!(
            (p[0].kind, p[0].first_lba, p[0].sectors),
            (0x0C, 2048, 30720)
        );
        assert_eq!(p[0].blocks(), Some((256, 3840)));
        assert_eq!(p[1].index, 2);
        // Not aligned to 4 KiB.
        let s = sector(&[(0, 0x06, 63, 1000)]);
        assert_eq!(read(&s).expect("a table")[0].blocks(), None);
        // A protective MBR, no signature, a FAT boot sector.
        assert!(read(&sector(&[(0, 0xEE, 1, 100)])).is_none());
        assert!(read(&vec![0u8; 512]).is_none());
        assert!(read(&sector(&[(0x12, 0x0C, 2048, 100)])).is_none());
    }
}
