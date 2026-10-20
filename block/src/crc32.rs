// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! CRC-32 (IEEE 802.3, reflected polynomial 0xEDB88320), as GPT uses it
//! (UEFI Specification 2.10, 5.3.2).

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
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

/// The CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    !data
        .iter()
        .fold(!0u32, |c, &b| TABLE[usize::from((c as u8) ^ b)] ^ (c >> 8))
}

#[cfg(test)]
mod tests {
    #[test]
    fn check_value() {
        // The standard check value of CRC-32/ISO-HDLC.
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(super::crc32(b""), 0);
    }
}
