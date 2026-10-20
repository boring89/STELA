// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! CRC-32C (Castagnoli, reflected polynomial 0x82F63B78; RFC 3720
//! appendix B), the checksum of every eHFS metadata structure.

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0x82F6_3B78 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = table();

/// Continues the CRC `crc` (0 to start) over `data`: `crc32c(crc32c(0,
/// a), b) == crc32c(0, a ++ b)`.
pub fn crc32c(crc: u32, data: &[u8]) -> u32 {
    !data
        .iter()
        .fold(!crc, |c, &b| TABLE[usize::from((c as u8) ^ b)] ^ (c >> 8))
}

#[cfg(test)]
mod tests {
    use super::crc32c;

    #[test]
    fn standard_values() {
        // RFC 3720 B.4 test vectors and the "123456789" check value.
        assert_eq!(crc32c(0, b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(0, &[0u8; 32]), 0x8A91_36AA);
        assert_eq!(crc32c(0, &[0xFFu8; 32]), 0x62A8_AB43);
        let inc: Vec<u8> = (0..32).collect();
        assert_eq!(crc32c(0, &inc), 0x46DD_794E);
        // Chaining.
        assert_eq!(crc32c(crc32c(0, b"1234"), b"56789"), 0xE306_9283);
    }
}
