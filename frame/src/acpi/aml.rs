// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A minimal scan of the DSDT's AML for the `\_S5` object: the two
//! numbers (SLP_TYPa, SLP_TYPb) that put the machine into the S5 "soft
//! off" state. Pure logic, tested on the host with hand-made AML.
//!
//! STELA has no AML interpreter. The firmware almost always declares
//! `\_S5` as a plain named package, which compiles to
//!
//! ```text
//!  08            NameOp
//!  [5C]          optional root prefix '\'
//!  5F 53 35 5F   "_S5_"
//!  12            PackageOp
//!  pp [pp..]     PkgLength: bits 7-6 of the first byte = how many more
//!                bytes follow (0-3)
//!  nn            NumElements
//!  0A tt | tt    SLP_TYPa: BytePrefix + byte, or ZeroOp (00) / OneOp (01)
//!  0A tt | tt    SLP_TYPb
//!  ...
//! ```
//!
//! (ACPI Specification 6.5, 20.2.5 "Term Objects Encoding": NameOp 0x08,
//! PackageOp 0x12, BytePrefix 0x0A, ZeroOp 0x00, OneOp 0x01; 20.2.4
//! "Package Length Encoding"; 7.4.2 "\_Sx (System States)".) ZeroOp and
//! OneOp happen to have the values they stand for, so a bare byte is read
//! as the number itself. A `\_S5` computed by a method is not found, and
//! STELA then cannot power off through ACPI. The same approach as
//! 68k_HAL's `acpi_find_s5`.

/// AML opcodes used by the scan.
const NAME_OP: u8 = 0x08;
const ROOT_PREFIX: u8 = b'\\';
const PACKAGE_OP: u8 = 0x12;
const BYTE_PREFIX: u8 = 0x0A;
const ZERO_OP: u8 = 0x00;
const ONE_OP: u8 = 0x01;

/// Size of the table header in front of the AML.
const HEADER_LEN: usize = 36;

/// One integer element at `at`: (value, next offset).
fn element(aml: &[u8], at: usize) -> Option<(u8, usize)> {
    match *aml.get(at)? {
        BYTE_PREFIX => Some((*aml.get(at + 1)?, at + 2)),
        v @ (ZERO_OP | ONE_OP) => Some((v, at + 1)),
        _ => None,
    }
}

/// The package after `_S5_` at `at` (just past the name).
fn package(aml: &[u8], at: usize) -> Option<(u8, u8)> {
    if *aml.get(at)? != PACKAGE_OP {
        return None;
    }
    let lead = *aml.get(at + 1)?;
    let at = at + 2 + usize::from(lead >> 6);
    let count = *aml.get(at)?;
    if count < 1 {
        return None;
    }
    let (a, next) = element(aml, at + 1)?;
    // A one-element package: SLP_TYPb = SLP_TYPa.
    let b = if count >= 2 { element(aml, next)?.0 } else { a };
    Some((a & 7, b & 7))
}

/// SLP_TYPa and SLP_TYPb for S5 from a DSDT (header included), or `None`
/// when no plain `Name (_S5, Package ...)` is found.
pub fn find_s5(dsdt: &[u8]) -> Option<(u8, u8)> {
    let aml = dsdt.get(HEADER_LEN..)?;
    (0..aml.len().saturating_sub(3)).find_map(|i| {
        if &aml[i..i + 4] != b"_S5_" {
            return None;
        }
        let named = (i >= 1 && aml[i - 1] == NAME_OP)
            || (i >= 2 && aml[i - 2] == NAME_OP && aml[i - 1] == ROOT_PREFIX);
        if !named {
            return None;
        }
        package(aml, i + 4)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn dsdt(aml: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; HEADER_LEN];
        t[..4].copy_from_slice(b"DSDT");
        t.extend_from_slice(aml);
        t
    }

    #[test]
    fn byte_prefixed_values() {
        // Name (\_S5, Package (0x04) { 0x05, 0x05, Zero, Zero })
        let aml = [
            0x10,
            0x20,
            b'\\',
            b'_',
            b'S',
            b'B',
            b'_', // unrelated scope
            NAME_OP,
            ROOT_PREFIX,
            b'_',
            b'S',
            b'5',
            b'_',
            PACKAGE_OP,
            0x0A,
            0x04,
            BYTE_PREFIX,
            0x05,
            BYTE_PREFIX,
            0x05,
            ZERO_OP,
            ZERO_OP,
        ];
        assert_eq!(find_s5(&dsdt(&aml)), Some((5, 5)));
    }

    #[test]
    fn zero_and_one_ops_without_root_prefix() {
        // Name (_S5, Package (0x02) { Zero, One }), QEMU's PIIX style.
        let aml = [
            NAME_OP, b'_', b'S', b'5', b'_', PACKAGE_OP, 0x05, 0x02, ZERO_OP, ONE_OP,
        ];
        assert_eq!(find_s5(&dsdt(&aml)), Some((0, 1)));
    }

    #[test]
    fn multi_byte_package_length() {
        // PkgLength with one extra byte (bits 7-6 = 01).
        let aml = [
            NAME_OP,
            b'_',
            b'S',
            b'5',
            b'_',
            PACKAGE_OP,
            0x40 | 0x06,
            0x00,
            0x04,
            BYTE_PREFIX,
            0x07,
            BYTE_PREFIX,
            0x07,
            ZERO_OP,
            ZERO_OP,
        ];
        assert_eq!(find_s5(&dsdt(&aml)), Some((7, 7)));
    }

    #[test]
    fn single_element_package() {
        let aml = [
            NAME_OP,
            b'_',
            b'S',
            b'5',
            b'_',
            PACKAGE_OP,
            0x04,
            0x01,
            BYTE_PREFIX,
            0x02,
        ];
        assert_eq!(find_s5(&dsdt(&aml)), Some((2, 2)));
    }

    #[test]
    fn not_found() {
        // A method reference, not a Name.
        let aml = [0x14, b'_', b'S', b'5', b'_', PACKAGE_OP, 0x04, 0x01, 0x00];
        assert_eq!(find_s5(&dsdt(&aml)), None);
        // _S4_ only.
        let aml = [
            NAME_OP, b'_', b'S', b'4', b'_', PACKAGE_OP, 0x04, 0x02, ZERO_OP, ZERO_OP,
        ];
        assert_eq!(find_s5(&dsdt(&aml)), None);
        // Truncated package.
        let aml = [NAME_OP, b'_', b'S', b'5', b'_', PACKAGE_OP, 0x04];
        assert_eq!(find_s5(&dsdt(&aml)), None);
        // A computed element (a method call) is not understood.
        let aml = [
            NAME_OP, b'_', b'S', b'5', b'_', PACKAGE_OP, 0x05, 0x02, b'X', b'Y',
        ];
        assert_eq!(find_s5(&dsdt(&aml)), None);
        assert_eq!(find_s5(&[0u8; 10]), None);
    }

    #[test]
    fn a_later_match_is_used_when_the_first_is_not_a_package() {
        let aml = [
            b'_',
            b'S',
            b'5',
            b'_',
            0x00, // a bare reference
            NAME_OP,
            b'_',
            b'S',
            b'5',
            b'_',
            PACKAGE_OP,
            0x05,
            0x02,
            BYTE_PREFIX,
            0x03,
            BYTE_PREFIX,
            0x04,
        ];
        assert_eq!(find_s5(&dsdt(&aml)), Some((3, 4)));
    }
}
