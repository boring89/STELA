// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Parsing of the ACPI tables STELA needs, from plain bytes: the RSDP, the
//! root table (XSDT or RSDT) and the MADT. Pure logic, tested on the host
//! with hand-made tables; `acpi/mod.rs` finds the bytes in memory.
//!
//! References (ACPI Specification 6.5):
//! - 5.2.5.3 "Root System Description Pointer (RSDP) Structure", Table 5.3
//! - 5.2.6 "System Description Table Header", Table 5.4
//! - 5.2.7 "Root System Description Table (RSDT)", 5.2.8 "Extended System
//!   Description Table (XSDT)"
//! - 5.2.12 "Multiple APIC Description Table (MADT)", Tables 5.19-5.32
//!
//! All multi-byte fields are little endian. A table is valid when its bytes
//! (`length` of them) add up to 0 modulo 256.

use alloc::vec::Vec;

/// Why a table was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpiError {
    /// The signature is not the expected one.
    BadSignature,
    /// The bytes do not add up to 0.
    BadChecksum,
    /// The length field is smaller than the header or larger than the data.
    BadLength,
    /// An MADT entry runs past the end of the table or is too short.
    BadEntry,
}

/// Sum of `bytes` modulo 256; 0 for a valid table.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, &b| sum.wrapping_add(b))
}

fn u8_at(bytes: &[u8], offset: usize) -> Result<u8, AcpiError> {
    bytes.get(offset).copied().ok_or(AcpiError::BadLength)
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, AcpiError> {
    let b = bytes.get(offset..offset + 2).ok_or(AcpiError::BadLength)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, AcpiError> {
    let b = bytes.get(offset..offset + 4).ok_or(AcpiError::BadLength)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, AcpiError> {
    let b = bytes.get(offset..offset + 8).ok_or(AcpiError::BadLength)?;
    let mut word = [0; 8];
    word.copy_from_slice(b);
    Ok(u64::from_le_bytes(word))
}

/// Size of the ACPI 1.0 RSDP (checksum covers these bytes).
pub const RSDP_V1_LEN: usize = 20;
/// Size of the ACPI 2.0+ RSDP (extended checksum covers these bytes).
pub const RSDP_V2_LEN: usize = 36;

/// What the RSDP points to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rsdp {
    /// 0 for ACPI 1.0, 2 for ACPI 2.0 and later.
    pub revision: u8,
    /// Physical address of the RSDT (32-bit entries).
    pub rsdt: u32,
    /// Physical address of the XSDT (64-bit entries), revision 2+ only.
    pub xsdt: Option<u64>,
}

impl Rsdp {
    /// The root table to use: the XSDT when there is one (`true` = 64-bit
    /// entries), else the RSDT.
    pub fn root(&self) -> (u64, bool) {
        match self.xsdt {
            Some(xsdt) if xsdt != 0 => (xsdt, true),
            _ => (u64::from(self.rsdt), false),
        }
    }
}

/// Parses an RSDP (`bytes` may be longer than the structure). Offsets:
/// 0 signature "RSD PTR ", 8 checksum, 9 OEM id, 15 revision, 16 RSDT
/// address; revision 2+: 20 length, 24 XSDT address, 32 extended checksum.
pub fn parse_rsdp(bytes: &[u8]) -> Result<Rsdp, AcpiError> {
    let v1 = bytes.get(..RSDP_V1_LEN).ok_or(AcpiError::BadLength)?;
    if &v1[..8] != b"RSD PTR " {
        return Err(AcpiError::BadSignature);
    }
    if checksum(v1) != 0 {
        return Err(AcpiError::BadChecksum);
    }
    let revision = v1[15];
    let rsdt = u32_at(v1, 16)?;
    if revision < 2 {
        return Ok(Rsdp {
            revision,
            rsdt,
            xsdt: None,
        });
    }
    let length = u32_at(bytes, 20)? as usize;
    if length < RSDP_V2_LEN {
        return Err(AcpiError::BadLength);
    }
    let v2 = bytes.get(..length).ok_or(AcpiError::BadLength)?;
    if checksum(v2) != 0 {
        return Err(AcpiError::BadChecksum);
    }
    Ok(Rsdp {
        revision,
        rsdt,
        xsdt: Some(u64_at(v2, 24)?),
    })
}

/// Size of the common table header.
pub const HEADER_LEN: usize = 36;

/// The 4-byte signature and the `length` field (offset 4) of a table
/// header; enough to decide how many bytes to read.
pub fn header_info(bytes: &[u8]) -> Result<([u8; 4], usize), AcpiError> {
    let header = bytes.get(..HEADER_LEN).ok_or(AcpiError::BadLength)?;
    let mut signature = [0; 4];
    signature.copy_from_slice(&header[..4]);
    Ok((signature, u32_at(header, 4)? as usize))
}

/// Checks a whole table (header + body): its signature, that `length` fits
/// in `bytes` and the checksum. Returns the table's bytes (`length` long).
pub fn check_table<'a>(bytes: &'a [u8], signature: &[u8; 4]) -> Result<&'a [u8], AcpiError> {
    let (found, length) = header_info(bytes)?;
    if &found != signature {
        return Err(AcpiError::BadSignature);
    }
    if length < HEADER_LEN {
        return Err(AcpiError::BadLength);
    }
    let table = bytes.get(..length).ok_or(AcpiError::BadLength)?;
    if checksum(table) != 0 {
        return Err(AcpiError::BadChecksum);
    }
    Ok(table)
}

/// The physical addresses listed in a checked XSDT (`wide`, 8 bytes each)
/// or RSDT (4 bytes each).
pub fn root_entries(table: &[u8], wide: bool) -> impl Iterator<Item = u64> + '_ {
    let size = if wide { 8 } else { 4 };
    table[HEADER_LEN.min(table.len())..]
        .chunks_exact(size)
        .map(move |entry| {
            if wide {
                u64_at(entry, 0).unwrap_or(0)
            } else {
                u64::from(u32_at(entry, 0).unwrap_or(0))
            }
        })
}

/// Interrupt pin polarity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Polarity {
    /// Asserted when high.
    ActiveHigh,
    /// Asserted when low.
    ActiveLow,
}

/// Interrupt trigger mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// One interrupt per rising (or falling) edge.
    Edge,
    /// Asserted for as long as the level holds.
    Level,
}

/// MADT type 0, Processor Local APIC (Table 5.23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Processor {
    /// ACPI processor UID.
    pub processor_id: u8,
    /// The processor's local APIC id.
    pub apic_id: u8,
    /// Flags bit 0: usable now.
    pub enabled: bool,
    /// Flags bit 1: can be brought online later (only if not enabled).
    pub online_capable: bool,
}

/// MADT type 1, I/O APIC (Table 5.24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoApic {
    /// I/O APIC id.
    pub id: u8,
    /// Physical address of its registers.
    pub address: u32,
    /// The first global system interrupt (GSI) of its inputs.
    pub gsi_base: u32,
}

/// MADT type 2, Interrupt Source Override (Table 5.25): ISA IRQ `source`
/// is wired to `gsi`, with the given polarity and trigger mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptOverride {
    /// Always 0 (ISA).
    pub bus: u8,
    /// ISA IRQ number.
    pub source: u8,
    /// Global system interrupt it is connected to.
    pub gsi: u32,
    /// MPS INTI flags (Table 5.26): bits 0-1 polarity, bits 2-3 trigger.
    pub flags: u16,
}

impl InterruptOverride {
    /// Polarity; "conforms to the bus" means active high for ISA.
    pub fn polarity(&self) -> Polarity {
        if self.flags & 0b11 == 0b11 {
            Polarity::ActiveLow
        } else {
            Polarity::ActiveHigh
        }
    }

    /// Trigger mode; "conforms to the bus" means edge for ISA.
    pub fn trigger(&self) -> Trigger {
        if (self.flags >> 2) & 0b11 == 0b11 {
            Trigger::Level
        } else {
            Trigger::Edge
        }
    }
}

/// MADT type 4, Local APIC NMI (Table 5.29): which LINT pin of which
/// processor(s) carries NMI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalApicNmi {
    /// ACPI processor UID, or 0xFF for all processors.
    pub processor_id: u8,
    /// MPS INTI flags, as in [`InterruptOverride::flags`].
    pub flags: u16,
    /// LINT0 or LINT1.
    pub lint: u8,
}

/// What STELA uses from the MADT.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Madt {
    /// Physical address of the local APIC registers (type 5 overrides the
    /// 32-bit header field).
    pub local_apic_address: u64,
    /// Flags bit 0 (PCAT_COMPAT): the system also has dual 8259 PICs,
    /// which must be masked when using the APIC.
    pub pcat_compat: bool,
    /// Type 0 entries.
    pub processors: Vec<Processor>,
    /// Type 1 entries.
    pub io_apics: Vec<IoApic>,
    /// Type 2 entries.
    pub overrides: Vec<InterruptOverride>,
    /// Type 4 entries.
    pub nmis: Vec<LocalApicNmi>,
}

impl Madt {
    /// Where ISA IRQ `irq` arrives: GSI, polarity and trigger. Without an
    /// override the IRQ is identity mapped, active high and edge triggered.
    pub fn isa_irq(&self, irq: u8) -> (u32, Polarity, Trigger) {
        match self
            .overrides
            .iter()
            .find(|o| o.bus == 0 && o.source == irq)
        {
            Some(o) => (o.gsi, o.polarity(), o.trigger()),
            None => (u32::from(irq), Polarity::ActiveHigh, Trigger::Edge),
        }
    }

    /// The I/O APIC that has input `gsi`, given each I/O APIC's input
    /// count (`inputs(id)`, read from its version register).
    pub fn io_apic_for(&self, gsi: u32, inputs: impl Fn(&IoApic) -> u32) -> Option<&IoApic> {
        self.io_apics
            .iter()
            .find(|a| gsi >= a.gsi_base && gsi - a.gsi_base < inputs(a))
    }
}

/// Parses a checked MADT (signature "APIC"; see [`check_table`]). Offsets:
/// 36 local APIC address (u32), 40 flags, 44 entries, each starting with a
/// type byte and a length byte. Unknown entry types are skipped.
pub fn parse_madt(table: &[u8]) -> Result<Madt, AcpiError> {
    let mut madt = Madt {
        local_apic_address: u64::from(u32_at(table, 36)?),
        pcat_compat: u32_at(table, 40)? & 1 != 0,
        ..Madt::default()
    };
    let mut offset = 44;
    while offset < table.len() {
        let kind = u8_at(table, offset)?;
        let length = usize::from(u8_at(table, offset + 1).map_err(|_| AcpiError::BadEntry)?);
        let entry = table
            .get(offset..offset + length)
            .filter(|_| length >= 2)
            .ok_or(AcpiError::BadEntry)?;
        let need = |n: usize| {
            if length < n {
                Err(AcpiError::BadEntry)
            } else {
                Ok(())
            }
        };
        match kind {
            0 => {
                need(8)?;
                let flags = u32_at(entry, 4)?;
                madt.processors.push(Processor {
                    processor_id: entry[2],
                    apic_id: entry[3],
                    enabled: flags & 1 != 0,
                    online_capable: flags & 2 != 0,
                });
            }
            1 => {
                need(12)?;
                madt.io_apics.push(IoApic {
                    id: entry[2],
                    address: u32_at(entry, 4)?,
                    gsi_base: u32_at(entry, 8)?,
                });
            }
            2 => {
                need(10)?;
                madt.overrides.push(InterruptOverride {
                    bus: entry[2],
                    source: entry[3],
                    gsi: u32_at(entry, 4)?,
                    flags: u16_at(entry, 8)?,
                });
            }
            4 => {
                need(6)?;
                madt.nmis.push(LocalApicNmi {
                    processor_id: entry[2],
                    flags: u16_at(entry, 3)?,
                    lint: entry[5],
                });
            }
            5 => {
                need(12)?;
                madt.local_apic_address = u64_at(entry, 4)?;
            }
            _ => {}
        }
        offset += length;
    }
    Ok(madt)
}

/// One ECAM window of the MCFG: the configuration space of buses
/// `start_bus..=end_bus` of PCI segment `segment` starts at `base` (bus `b`,
/// device `d`, function `f` at `base + ((b - start_bus) << 20 | d << 15 |
/// f << 12)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct McfgEntry {
    /// Physical base address of the window (for bus `start_bus`).
    pub base: u64,
    /// PCI segment group.
    pub segment: u16,
    /// First bus decoded.
    pub start_bus: u8,
    /// Last bus decoded.
    pub end_bus: u8,
}

/// Parses a checked MCFG (signature "MCFG"; PCI Firmware Specification 3.0
/// section 4.1.2): 8 reserved bytes after the header, then 16-byte entries
/// (base u64, segment u16, start bus, end bus, 4 reserved).
pub fn parse_mcfg(table: &[u8]) -> Result<Vec<McfgEntry>, AcpiError> {
    let body = table.get(HEADER_LEN + 8..).ok_or(AcpiError::BadLength)?;
    if !body.len().is_multiple_of(16) {
        return Err(AcpiError::BadEntry);
    }
    let mut entries = Vec::new();
    for entry in body.as_chunks::<16>().0 {
        let e = McfgEntry {
            base: u64_at(entry, 0)?,
            segment: u16_at(entry, 8)?,
            start_bus: entry[10],
            end_bus: entry[11],
        };
        if e.end_bus < e.start_bus {
            return Err(AcpiError::BadEntry);
        }
        entries.push(e);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcfg_entries() {
        // QEMU q35: one window at 0xB000_0000, segment 0, buses 0-255.
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&0xB000_0000u64.to_le_bytes());
        body.extend_from_slice(&[0, 0, 0, 0xFF, 0, 0, 0, 0]);
        let entries = parse_mcfg(&table(b"MCFG", &body)).unwrap();
        assert_eq!(
            entries,
            [McfgEntry {
                base: 0xB000_0000,
                segment: 0,
                start_bus: 0,
                end_bus: 255
            }]
        );
        assert_eq!(parse_mcfg(&table(b"MCFG", &[0; 8])).unwrap(), []);
        assert_eq!(
            parse_mcfg(&table(b"MCFG", &[0; 12])),
            Err(AcpiError::BadEntry)
        );
        let mut bad = vec![0u8; 8];
        bad.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 4, 0, 0, 0, 0]);
        assert_eq!(parse_mcfg(&table(b"MCFG", &bad)), Err(AcpiError::BadEntry));
    }

    /// Sets byte `at` so that the whole slice sums to 0.
    fn fix_checksum(bytes: &mut [u8], at: usize) {
        bytes[at] = 0;
        bytes[at] = 0u8.wrapping_sub(checksum(bytes));
    }

    fn rsdp_v2(xsdt: u64) -> Vec<u8> {
        let mut b = vec![0u8; 36];
        b[..8].copy_from_slice(b"RSD PTR ");
        b[9..15].copy_from_slice(b"STELA ");
        b[15] = 2;
        b[16..20].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        b[20..24].copy_from_slice(&36u32.to_le_bytes());
        b[24..32].copy_from_slice(&xsdt.to_le_bytes());
        fix_checksum(&mut b[..20], 8);
        fix_checksum(&mut b, 32);
        b
    }

    /// A table with a header and `body`, checksum fixed.
    fn table(signature: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; HEADER_LEN];
        t[..4].copy_from_slice(signature);
        t[4..8].copy_from_slice(&((HEADER_LEN + body.len()) as u32).to_le_bytes());
        t[8] = 1;
        t[10..16].copy_from_slice(b"STELA ");
        t.extend_from_slice(body);
        fix_checksum(&mut t, 9);
        t
    }

    /// The MADT QEMU's q35 machine reports (one CPU), plus an NMI entry and
    /// a local APIC address override.
    fn madt_body(with_override: bool) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes()); // PCAT_COMPAT
        b.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]); // CPU 0, APIC id 0, enabled
        b.extend_from_slice(&[0, 8, 1, 1, 2, 0, 0, 0]); // CPU 1, online capable
        b.extend_from_slice(&[1, 12, 0, 0]); // I/O APIC id 0
        b.extend_from_slice(&0xFEC0_0000u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&[2, 10, 0, 0, 2, 0, 0, 0, 0, 0]); // IRQ0 -> GSI 2
        b.extend_from_slice(&[2, 10, 0, 9, 9, 0, 0, 0, 0x0D, 0]); // IRQ9 level, high
        b.extend_from_slice(&[2, 10, 0, 11, 11, 0, 0, 0, 0x0F, 0]); // IRQ11 level, low
        b.extend_from_slice(&[4, 6, 0xFF, 0x05, 0x00, 1]); // NMI on LINT1
        b.extend_from_slice(&[9, 3, 0]); // unknown type: skipped
        if with_override {
            b.extend_from_slice(&[5, 12, 0, 0]);
            b.extend_from_slice(&0x1_FEE0_0000u64.to_le_bytes());
        }
        b
    }

    #[test]
    fn rsdp_revision_2_uses_the_xsdt() {
        let rsdp = parse_rsdp(&rsdp_v2(0x7FE1_4000)).unwrap();
        assert_eq!(rsdp.revision, 2);
        assert_eq!(rsdp.rsdt, 0x1234_5678);
        assert_eq!(rsdp.root(), (0x7FE1_4000, true));
    }

    #[test]
    fn rsdp_revision_0_uses_the_rsdt() {
        let mut b = rsdp_v2(0);
        b[15] = 0;
        fix_checksum(&mut b[..20], 8);
        let rsdp = parse_rsdp(&b[..20]).unwrap();
        assert_eq!(rsdp.xsdt, None);
        assert_eq!(rsdp.root(), (0x1234_5678, false));
    }

    #[test]
    fn rsdp_errors() {
        let mut b = rsdp_v2(0x1000);
        b[0] = b'X';
        assert_eq!(parse_rsdp(&b), Err(AcpiError::BadSignature));
        let mut b = rsdp_v2(0x1000);
        b[16] ^= 1;
        assert_eq!(parse_rsdp(&b), Err(AcpiError::BadChecksum));
        let mut b = rsdp_v2(0x1000);
        b[24] ^= 1; // only the extended checksum notices
        assert_eq!(parse_rsdp(&b), Err(AcpiError::BadChecksum));
        assert_eq!(parse_rsdp(&rsdp_v2(1)[..30]), Err(AcpiError::BadLength));
        assert_eq!(parse_rsdp(b"RSD PTR "), Err(AcpiError::BadLength));
    }

    #[test]
    fn root_tables_list_addresses() {
        let mut body = Vec::new();
        body.extend_from_slice(&0x1_0000_1000u64.to_le_bytes());
        body.extend_from_slice(&0x2000u64.to_le_bytes());
        let xsdt = table(b"XSDT", &body);
        let checked = check_table(&xsdt, b"XSDT").unwrap();
        let entries: Vec<u64> = root_entries(checked, true).collect();
        assert_eq!(entries, [0x1_0000_1000, 0x2000]);

        let rsdt = table(b"RSDT", &[0x00, 0x10, 0, 0, 0x00, 0x20, 0, 0]);
        let entries: Vec<u64> = root_entries(check_table(&rsdt, b"RSDT").unwrap(), false).collect();
        assert_eq!(entries, [0x1000, 0x2000]);
    }

    #[test]
    fn table_errors() {
        let good = table(b"APIC", &madt_body(false));
        assert_eq!(check_table(&good, b"XSDT"), Err(AcpiError::BadSignature));
        let mut bad = good.clone();
        bad[40] ^= 0x80;
        assert_eq!(check_table(&bad, b"APIC"), Err(AcpiError::BadChecksum));
        // Length larger than the bytes, or smaller than the header.
        assert_eq!(
            check_table(&good[..good.len() - 1], b"APIC"),
            Err(AcpiError::BadLength)
        );
        let mut short = good.clone();
        short[4..8].copy_from_slice(&20u32.to_le_bytes());
        assert_eq!(check_table(&short, b"APIC"), Err(AcpiError::BadLength));
        assert_eq!(check_table(&good[..10], b"APIC"), Err(AcpiError::BadLength));
        // Extra bytes after the table are ignored.
        let mut longer = good.clone();
        longer.extend_from_slice(&[1, 2, 3]);
        assert_eq!(check_table(&longer, b"APIC").unwrap().len(), good.len());
    }

    #[test]
    fn madt_entries() {
        let bytes = table(b"APIC", &madt_body(false));
        let madt = parse_madt(check_table(&bytes, b"APIC").unwrap()).unwrap();
        assert_eq!(madt.local_apic_address, 0xFEE0_0000);
        assert!(madt.pcat_compat);
        assert_eq!(madt.processors.len(), 2);
        assert_eq!(
            madt.processors[0],
            Processor {
                processor_id: 0,
                apic_id: 0,
                enabled: true,
                online_capable: false
            }
        );
        assert!(!madt.processors[1].enabled && madt.processors[1].online_capable);
        assert_eq!(
            madt.io_apics,
            [IoApic {
                id: 0,
                address: 0xFEC0_0000,
                gsi_base: 0
            }]
        );
        assert_eq!(madt.overrides.len(), 3);
        assert_eq!(
            madt.nmis,
            [LocalApicNmi {
                processor_id: 0xFF,
                flags: 5,
                lint: 1
            }]
        );
    }

    #[test]
    fn madt_isa_irq_routing() {
        let madt = parse_madt(&table(b"APIC", &madt_body(false))).unwrap();
        assert_eq!(madt.isa_irq(0), (2, Polarity::ActiveHigh, Trigger::Edge));
        assert_eq!(madt.isa_irq(1), (1, Polarity::ActiveHigh, Trigger::Edge));
        assert_eq!(madt.isa_irq(9), (9, Polarity::ActiveHigh, Trigger::Level));
        assert_eq!(madt.isa_irq(11), (11, Polarity::ActiveLow, Trigger::Level));
        assert_eq!(madt.io_apic_for(2, |_| 24).map(|a| a.id), Some(0));
        assert_eq!(madt.io_apic_for(24, |_| 24), None);
    }

    #[test]
    fn madt_address_override() {
        let madt = parse_madt(&table(b"APIC", &madt_body(true))).unwrap();
        assert_eq!(madt.local_apic_address, 0x1_FEE0_0000);
    }

    #[test]
    fn madt_bad_entries() {
        let mut body = madt_body(false);
        body.extend_from_slice(&[1, 12, 0, 0]); // truncated I/O APIC entry
        assert_eq!(parse_madt(&table(b"APIC", &body)), Err(AcpiError::BadEntry));
        let mut body = madt_body(false);
        body.extend_from_slice(&[0, 0]); // zero length would loop forever
        assert_eq!(parse_madt(&table(b"APIC", &body)), Err(AcpiError::BadEntry));
        let mut body = madt_body(false);
        body.extend_from_slice(&[1, 4, 0, 0]); // too short for its type
        assert_eq!(parse_madt(&table(b"APIC", &body)), Err(AcpiError::BadEntry));
        let mut body = madt_body(false);
        body.push(0); // lone type byte without a length
        assert_eq!(parse_madt(&table(b"APIC", &body)), Err(AcpiError::BadEntry));
    }
}
