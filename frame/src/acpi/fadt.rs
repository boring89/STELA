// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Fixed ACPI Description Table (FADT, signature "FACP"): where the
//! power management registers are, the DSDT, the reset register and the
//! CMOS century register. Pure logic, tested on the host.
//!
//! Field offsets (ACPI Specification 6.5, 5.2.9 "Fixed ACPI Description
//! Table", Table 5.9):
//!
//! ```text
//!  40  DSDT            u32   physical address of the DSDT
//!  48  SMI_CMD         u32   I/O port; writing ACPI_ENABLE switches to ACPI mode
//!  52  ACPI_ENABLE     u8
//!  64  PM1a_CNT_BLK    u32   I/O port of the PM1a control register
//!  68  PM1b_CNT_BLK    u32   (0 = none)
//! 108  CENTURY         u8    CMOS index of the century, 0 = none
//! 109  IAPC_BOOT_ARCH  u16   bit 1: an 8042 is present
//! 112  Flags           u32   bit 10 RESET_REG_SUP
//! 116  RESET_REG       GAS   12 bytes: space id, bit width, bit offset,
//!                            access size, address u64 (at 120)
//! 128  RESET_VALUE     u8
//! 140  X_DSDT          u64   (ACPI 2.0+; preferred when non-zero)
//! 172  X_PM1a_CNT_BLK  GAS   (used when PM1a_CNT_BLK is 0)
//! 184  X_PM1b_CNT_BLK  GAS
//! ```
//!
//! Old FADTs (ACPI 1.0) are 116 bytes long and end before the reset
//! register; the fields past the end are taken as absent.

use super::tables::{AcpiError, u8_at, u16_at, u32_at, u64_at};

/// Generic Address Structure space id: system memory.
pub const SPACE_MEMORY: u8 = 0;
/// Generic Address Structure space id: system I/O ports.
pub const SPACE_IO: u8 = 1;

/// FADT flag bit 10: the reset register is supported.
const RESET_REG_SUP: u32 = 1 << 10;
/// IAPC_BOOT_ARCH bit 1: the motherboard has an 8042 keyboard controller.
const BOOT_ARCH_8042: u16 = 1 << 1;

/// The ACPI reset register (Table 5.9 RESET_REG and RESET_VALUE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetRegister {
    /// [`SPACE_IO`] or [`SPACE_MEMORY`] (other spaces are not accepted).
    pub space: u8,
    /// Port number or physical address.
    pub address: u64,
    /// The byte to write.
    pub value: u8,
}

/// What STELA uses from the FADT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fadt {
    /// Physical address of the DSDT (X_DSDT when set), 0 = none.
    pub dsdt: u64,
    /// The SMI command port and the value that switches to ACPI mode
    /// (both 0 when the machine is always in ACPI mode).
    pub smi_command: u32,
    /// See `smi_command`.
    pub acpi_enable: u8,
    /// I/O port of the PM1a control register (0 = none).
    pub pm1a_control: u32,
    /// I/O port of the PM1b control register (0 = none).
    pub pm1b_control: u32,
    /// CMOS register index of the century (0 = none).
    pub century: u8,
    /// IAPC_BOOT_ARCH bit 1. On ACPI 1.0 tables (no such field) `true`.
    pub has_8042: bool,
    /// The reset register, when the flags say it is supported and it is in
    /// the I/O or memory space.
    pub reset: Option<ResetRegister>,
}

/// The I/O port of a GAS at `offset`, when it is in the I/O space and the
/// table is long enough; else 0.
fn gas_io_port(table: &[u8], offset: usize) -> u32 {
    match (u8_at(table, offset), u64_at(table, offset + 4)) {
        (Ok(SPACE_IO), Ok(port)) if port <= 0xFFFF => port as u32,
        _ => 0,
    }
}

/// Parses a checked FADT (signature "FACP", checksum already verified).
pub fn parse_fadt(table: &[u8]) -> Result<Fadt, AcpiError> {
    if table.get(..4) != Some(b"FACP") {
        return Err(AcpiError::BadSignature);
    }
    // Everything up to PM1b_CNT_BLK must be there.
    if table.len() < 72 {
        return Err(AcpiError::BadLength);
    }
    let x_dsdt = u64_at(table, 140).unwrap_or(0);
    let dsdt = if x_dsdt != 0 {
        x_dsdt
    } else {
        u64::from(u32_at(table, 40)?)
    };
    let mut pm1a_control = u32_at(table, 64)?;
    let mut pm1b_control = u32_at(table, 68)?;
    if pm1a_control == 0 {
        pm1a_control = gas_io_port(table, 172);
        pm1b_control = gas_io_port(table, 184);
    }
    let flags = u32_at(table, 112).unwrap_or(0);
    let reset = match (u8_at(table, 116), u64_at(table, 120), u8_at(table, 128)) {
        (Ok(space), Ok(address), Ok(value))
            if flags & RESET_REG_SUP != 0
                && (space == SPACE_IO || space == SPACE_MEMORY)
                && address != 0 =>
        {
            Some(ResetRegister {
                space,
                address,
                value,
            })
        }
        _ => None,
    };
    Ok(Fadt {
        dsdt,
        smi_command: u32_at(table, 48)?,
        acpi_enable: u8_at(table, 52)?,
        pm1a_control,
        pm1b_control,
        century: u8_at(table, 108).unwrap_or(0),
        has_8042: u16_at(table, 109).map_or(true, |arch| arch & BOOT_ARCH_8042 != 0),
        reset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// A FADT of `length` bytes with QEMU q35-like values.
    fn fadt(length: usize) -> Vec<u8> {
        let mut t = vec![0u8; length];
        t[..4].copy_from_slice(b"FACP");
        t[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        let mut put = |at: usize, bytes: &[u8]| {
            if at + bytes.len() <= length {
                t[at..at + bytes.len()].copy_from_slice(bytes);
            }
        };
        put(40, &0x7FE0_0000u32.to_le_bytes());
        put(48, &0xB2u32.to_le_bytes());
        put(52, &[0xF1]);
        put(64, &0x604u32.to_le_bytes());
        put(108, &[0x32]);
        put(109, &2u16.to_le_bytes());
        put(112, &RESET_REG_SUP.to_le_bytes());
        put(116, &[SPACE_IO, 8, 0, 1]);
        put(120, &0xCF9u64.to_le_bytes());
        put(128, &[0x0F]);
        t
    }

    #[test]
    fn full_fadt() {
        let f = parse_fadt(&fadt(244)).unwrap();
        assert_eq!(f.dsdt, 0x7FE0_0000);
        assert_eq!((f.smi_command, f.acpi_enable), (0xB2, 0xF1));
        assert_eq!((f.pm1a_control, f.pm1b_control), (0x604, 0));
        assert_eq!(f.century, 0x32);
        assert!(f.has_8042);
        assert_eq!(
            f.reset,
            Some(ResetRegister {
                space: SPACE_IO,
                address: 0xCF9,
                value: 0x0F
            })
        );
    }

    #[test]
    fn x_dsdt_wins() {
        let mut t = fadt(244);
        t[140..148].copy_from_slice(&0x1_2345_0000u64.to_le_bytes());
        assert_eq!(parse_fadt(&t).unwrap().dsdt, 0x1_2345_0000);
    }

    #[test]
    fn x_pm1_blocks_when_the_legacy_field_is_zero() {
        let mut t = fadt(244);
        t[64..68].fill(0);
        t[172..176].copy_from_slice(&[SPACE_IO, 16, 0, 2]);
        t[176..184].copy_from_slice(&0x1004u64.to_le_bytes());
        t[184..188].copy_from_slice(&[SPACE_MEMORY, 16, 0, 2]);
        t[188..196].copy_from_slice(&0xFED0_0000u64.to_le_bytes());
        let f = parse_fadt(&t).unwrap();
        assert_eq!((f.pm1a_control, f.pm1b_control), (0x1004, 0));
    }

    #[test]
    fn acpi_1_fadt_has_no_reset_register() {
        let f = parse_fadt(&fadt(116)).unwrap();
        assert_eq!(f.reset, None);
        assert_eq!(f.century, 0x32);
        assert_eq!(f.pm1a_control, 0x604);
    }

    #[test]
    fn reset_needs_the_flag_and_a_known_space() {
        let mut t = fadt(244);
        t[112..116].fill(0);
        assert_eq!(parse_fadt(&t).unwrap().reset, None);
        let mut t = fadt(244);
        t[116] = 2; // PCI configuration space
        assert_eq!(parse_fadt(&t).unwrap().reset, None);
    }

    #[test]
    fn no_8042_and_errors() {
        let mut t = fadt(244);
        t[109..111].fill(0);
        assert!(!parse_fadt(&t).unwrap().has_8042);
        assert_eq!(parse_fadt(&fadt(60)), Err(AcpiError::BadLength));
        let mut t = fadt(244);
        t[..4].copy_from_slice(b"APIC");
        assert_eq!(parse_fadt(&t), Err(AcpiError::BadSignature));
    }
}
