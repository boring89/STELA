// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the processor says about itself through CPUID (P11, for the
//! hardware report; Intel SDM Vol. 2A "CPUID—CPU Identification"). The
//! decoding is pure and takes a function that answers CPUID, so that it
//! can be tested on the host.

use alloc::string::String;
use alloc::vec::Vec;

/// CPUID's answer for (leaf, subleaf): EAX, EBX, ECX, EDX.
pub type Regs = [u32; 4];

/// The processor's identity and the features STELA cares about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CpuInfo {
    /// "GenuineIntel", "AuthenticAMD", ...
    pub vendor: String,
    /// The brand string (leaves 80000002H–80000004H), or empty.
    pub brand: String,
    /// Display family, model and stepping (leaf 01H EAX).
    pub family: u32,
    /// Display model.
    pub model: u32,
    /// Stepping.
    pub stepping: u32,
    /// Names of the features present, from [`FEATURES`].
    pub features: Vec<&'static str>,
}

/// Where a feature bit is: (name, leaf, subleaf, register 0–3 = EAX–EDX,
/// bit). Vol. 2A Tables 3-10, 3-11 and the leaf 07H, 80000001H and
/// 80000007H descriptions.
pub const FEATURES: [(&str, u32, u32, usize, u32); 12] = [
    ("sse2", 1, 0, 3, 26),
    ("sse3", 1, 0, 2, 0),
    ("ssse3", 1, 0, 2, 9),
    ("sse4.1", 1, 0, 2, 19),
    ("sse4.2", 1, 0, 2, 20),
    ("avx", 1, 0, 2, 28),
    ("avx2", 7, 0, 1, 5),
    ("x2apic", 1, 0, 2, 21),
    ("smep", 7, 0, 1, 7),
    ("nx", 0x8000_0001, 0, 3, 20),
    ("invariant-tsc", 0x8000_0007, 0, 3, 8),
    ("hypervisor", 1, 0, 2, 31),
];

/// Display family, model and stepping from leaf 01H's EAX: the extended
/// family is added when the family is 0FH, the extended model is put in
/// front when the family is 06H or 0FH (Vol. 2A, "CPUID", Figure 3-6).
pub fn signature(eax: u32) -> (u32, u32, u32) {
    let stepping = eax & 0xF;
    let base_model = (eax >> 4) & 0xF;
    let base_family = (eax >> 8) & 0xF;
    let ext_model = (eax >> 16) & 0xF;
    let ext_family = (eax >> 20) & 0xFF;
    let family = if base_family == 0xF {
        base_family + ext_family
    } else {
        base_family
    };
    let model = if base_family == 0x6 || base_family == 0xF {
        ext_model << 4 | base_model
    } else {
        base_model
    };
    (family, model, stepping)
}

/// The ASCII text in `words` (little-endian bytes), without NULs and the
/// spaces around it.
pub fn text(words: &[u32]) -> String {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let s: String = bytes
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '?'
            }
        })
        .collect();
    String::from(s.trim())
}

/// Decodes everything through `cpuid(leaf, subleaf)`.
pub fn decode(cpuid: impl Fn(u32, u32) -> Regs) -> CpuInfo {
    let [max, b, c, d] = cpuid(0, 0);
    let ext_max = cpuid(0x8000_0000, 0)[0];
    let present = |leaf: u32| {
        if leaf >= 0x8000_0000 {
            leaf <= ext_max
        } else {
            leaf <= max
        }
    };
    let (family, model, stepping) = signature(cpuid(1, 0)[0]);
    let brand = if ext_max >= 0x8000_0004 {
        let words: Vec<u32> = (0x8000_0002..=0x8000_0004u32)
            .flat_map(|leaf| cpuid(leaf, 0))
            .collect();
        text(&words)
    } else {
        String::new()
    };
    let features = FEATURES
        .iter()
        .filter(|(_, leaf, sub, reg, bit)| {
            present(*leaf) && cpuid(*leaf, *sub)[*reg] & (1 << bit) != 0
        })
        .map(|(name, ..)| *name)
        .collect();
    CpuInfo {
        vendor: text(&[b, d, c]),
        brand,
        family,
        model,
        stepping,
        features,
    }
}

/// The processor this runs on.
#[cfg(target_os = "none")]
pub fn info() -> CpuInfo {
    decode(|leaf, sub| {
        let r = core::arch::x86_64::__cpuid_count(leaf, sub);
        [r.eax, r.ebx, r.ecx, r.edx]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures() {
        // Skylake (06_5EH stepping 3): ext model 5, family 6, model E.
        assert_eq!(signature(0x0005_06E3), (6, 0x5E, 3));
        // AMD Zen 2 (17H): family F + ext 8.
        assert_eq!(signature(0x0083_0F10), (0x17, 0x31, 0));
        // Family 5: no extended model.
        assert_eq!(signature(0x0001_0543), (5, 4, 3));
    }

    #[test]
    fn decodes_a_processor() {
        let words = |s: &[u8]| -> [u32; 4] {
            let mut w = [0u32; 4];
            for (i, chunk) in s.chunks(4).enumerate() {
                let mut b = [0u8; 4];
                b[..chunk.len()].copy_from_slice(chunk);
                w[i] = u32::from_le_bytes(b);
            }
            w
        };
        let brand = b"  QEMU Virtual CPU version 2.5+\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
        let cpuid = |leaf: u32, _sub: u32| -> Regs {
            match leaf {
                // "GenuineIntel" is EBX, EDX, ECX.
                0 => [7, 0x756E_6547, 0x6C65_746E, 0x4965_6E69],
                1 => [0x0006_06E3, 0, 1 | 1 << 31, 1 << 26],
                7 => [0, 1 << 7, 0, 0],
                0x8000_0000 => [0x8000_0004, 0, 0, 0],
                0x8000_0001 => [0, 0, 0, 1 << 20],
                0x8000_0002 => words(&brand[0..16]),
                0x8000_0003 => words(&brand[16..32]),
                0x8000_0004 => words(&brand[32..48]),
                // Beyond 80000004H: must not be read.
                _ => [u32::MAX; 4],
            }
        };
        let info = decode(cpuid);
        assert_eq!(info.vendor, "GenuineIntel");
        assert_eq!(info.brand, "QEMU Virtual CPU version 2.5+");
        assert_eq!((info.family, info.model, info.stepping), (6, 0x6E, 3));
        assert_eq!(info.features, ["sse2", "sse3", "smep", "nx", "hypervisor"]);
    }

    #[test]
    fn old_processor_without_leaves() {
        let info = decode(|leaf, _| match leaf {
            0 => [1, 0, 0, 0],
            1 => [0x0000_0543, 0, 0, 1 << 26],
            0x8000_0000 => [0, 0, 0, 0],
            _ => [u32::MAX; 4],
        });
        assert_eq!(info.brand, "");
        assert_eq!(info.features, ["sse2"]);
    }
}
