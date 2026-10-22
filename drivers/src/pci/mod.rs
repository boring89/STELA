// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! PCI: enumeration, BARs, capabilities and MSI (`docs/pci.md`).
//!
//! Every PCI function has a 256-byte configuration space (4 KiB with
//! PCI Express); the first 64 bytes are a standard header (PCI Local Bus
//! Specification 3.0, chapter 6):
//!
//! ```text
//!  0x00 vendor id | device id      0x08 revision | prog if | subclass | class
//!  0x04 command   | status         0x0C ... | header type (bit 7: multi-function)
//!  0x10-0x24 BAR0-5 (type 0)       0x18 primary | secondary | subordinate bus (type 1)
//!  0x34 capabilities pointer       0x3C interrupt line | interrupt pin
//! ```
//!
//! The logic here works over the [`ConfigSpace`] trait, so it is tested on
//! the host with a fake bus; [`system`] provides the real access (ECAM
//! from the ACPI MCFG, or the legacy I/O ports).

#[cfg(target_os = "none")]
pub mod system;

use alloc::vec::Vec;

/// Where a function sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Address {
    /// PCI segment group (always 0 so far).
    pub segment: u16,
    /// Bus number.
    pub bus: u8,
    /// Device number (0-31).
    pub device: u8,
    /// Function number (0-7).
    pub function: u8,
}

impl core::fmt::Display for Address {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:04x}:{:02x}:{:02x}.{}",
            self.segment, self.bus, self.device, self.function
        )
    }
}

/// Access to configuration space: aligned 32-bit registers.
pub trait ConfigSpace {
    /// Reads the register at `offset` (a multiple of 4). A function that
    /// does not exist reads as all ones.
    fn read32(&self, at: Address, offset: u16) -> u32;
    /// Writes the register at `offset` (a multiple of 4).
    fn write32(&self, at: Address, offset: u16, value: u32);
    /// Whether some function answers at `at` (its vendor is not 0xFFFF),
    /// as cheaply as the access method allows (P12: the scan of every
    /// bus asks this first, so that ECAM windows are not mapped for empty
    /// buses).
    fn present(&self, at: Address) -> bool {
        self.read32(at, 0) & 0xFFFF != 0xFFFF
    }
}

/// Reads the 16-bit field at `offset` (a multiple of 2).
pub fn read16(cfg: &dyn ConfigSpace, at: Address, offset: u16) -> u16 {
    (cfg.read32(at, offset & !3) >> ((offset & 2) * 8)) as u16
}

/// Writes the 16-bit field at `offset` (a multiple of 2), keeping the
/// other half of the register.
pub fn write16(cfg: &dyn ConfigSpace, at: Address, offset: u16, value: u16) {
    let shift = (offset & 2) * 8;
    let old = cfg.read32(at, offset & !3);
    let new = (old & !(0xFFFF << shift)) | u32::from(value) << shift;
    cfg.write32(at, offset & !3, new);
}

/// Reads the byte at `offset`.
pub fn read8(cfg: &dyn ConfigSpace, at: Address, offset: u16) -> u8 {
    (cfg.read32(at, offset & !3) >> ((offset & 3) * 8)) as u8
}

/// Command register (0x04) bits.
pub mod command {
    /// Respond to I/O space accesses.
    pub const IO_SPACE: u16 = 1 << 0;
    /// Respond to memory space accesses.
    pub const MEMORY_SPACE: u16 = 1 << 1;
    /// May master the bus (DMA, MSI writes).
    pub const BUS_MASTER: u16 = 1 << 2;
    /// INTx# disabled.
    pub const INTX_DISABLE: u16 = 1 << 10;
}

/// Capability ids.
pub mod capability {
    /// Message Signaled Interrupts.
    pub const MSI: u8 = 0x05;
    /// PCI Express.
    pub const PCI_EXPRESS: u8 = 0x10;
    /// MSI-X.
    pub const MSI_X: u8 = 0x11;
}

/// A function found by [`scan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    /// Its address.
    pub address: Address,
    /// Vendor id.
    pub vendor: u16,
    /// Device id.
    pub device: u16,
    /// Base class.
    pub class: u8,
    /// Subclass.
    pub subclass: u8,
    /// Programming interface.
    pub prog_if: u8,
    /// Header type without the multi-function bit (0 device, 1 bridge).
    pub header_type: u8,
}

impl Function {
    /// Whether this is class `class`, subclass `subclass`, interface
    /// `prog_if`.
    pub fn is(&self, class: u8, subclass: u8, prog_if: u8) -> bool {
        (self.class, self.subclass, self.prog_if) == (class, subclass, prog_if)
    }

    /// A short description of well-known classes.
    pub fn kind(&self) -> &'static str {
        match (self.class, self.subclass, self.prog_if) {
            (0x01, 0x01, _) => "IDE controller",
            (0x01, 0x06, 0x01) => "AHCI controller",
            (0x01, 0x08, 0x02) => "NVMe controller",
            (0x01, _, _) => "storage controller",
            (0x02, _, _) => "network controller",
            (0x03, _, _) => "display controller",
            (0x04, _, _) => "multimedia controller",
            (0x06, 0x00, _) => "host bridge",
            (0x06, 0x01, _) => "ISA bridge",
            (0x06, 0x04, _) => "PCI bridge",
            (0x06, _, _) => "bridge",
            (0x0C, 0x03, _) => "USB controller",
            (0x0C, 0x05, _) => "SMBus controller",
            _ => "device",
        }
    }
}

/// Reads the header of the function at `at`, if one is there.
pub fn probe(cfg: &dyn ConfigSpace, at: Address) -> Option<Function> {
    let id = cfg.read32(at, 0x00);
    let vendor = id as u16;
    if vendor == 0xFFFF || vendor == 0 {
        return None;
    }
    let class = cfg.read32(at, 0x08);
    Some(Function {
        address: at,
        vendor,
        device: (id >> 16) as u16,
        class: (class >> 24) as u8,
        subclass: (class >> 16) as u8,
        prog_if: (class >> 8) as u8,
        header_type: read8(cfg, at, 0x0E) & 0x7F,
    })
}

/// Every function on the segment, by walking bus `root` and every bus
/// behind a PCI-to-PCI bridge (OSDev Wiki "PCI", "Recursive Scan"). A
/// multi-function host bridge at 00.0 means several root buses: function
/// `f` of it is the controller of bus `f`. P12: then every bus 0-255 not
/// seen yet is scanned too ("Brute Force Scan"): a machine with several
/// root complexes (the P520: NVMe on bus 0x17, the GPU on bus 0x65) has
/// buses no bridge on bus 0 leads to.
pub fn scan(cfg: &dyn ConfigSpace, segment: u16) -> Vec<Function> {
    let mut found = Vec::new();
    let mut visited = [false; 256];
    let root = Address {
        segment,
        ..Address::default()
    };
    let multi_root = read8(cfg, root, 0x0E) & 0x80 != 0;
    if multi_root && probe(cfg, root).is_some() {
        for function in 0..8 {
            let at = Address { function, ..root };
            if probe(cfg, at).is_some() {
                scan_bus(cfg, segment, function, &mut visited, &mut found);
            }
        }
    } else {
        scan_bus(cfg, segment, 0, &mut visited, &mut found);
    }
    for bus in 0..=255u8 {
        let occupied = (0..32).any(|device| {
            cfg.present(Address {
                segment,
                bus,
                device,
                function: 0,
            })
        });
        if occupied {
            scan_bus(cfg, segment, bus, &mut visited, &mut found);
        }
    }
    found.sort_by_key(|f| f.address);
    found
}

fn scan_bus(
    cfg: &dyn ConfigSpace,
    segment: u16,
    bus: u8,
    visited: &mut [bool; 256],
    found: &mut Vec<Function>,
) {
    if core::mem::replace(&mut visited[usize::from(bus)], true) {
        return;
    }
    for device in 0..32 {
        let first = Address {
            segment,
            bus,
            device,
            function: 0,
        };
        let Some(f0) = probe(cfg, first) else {
            continue;
        };
        let functions = if read8(cfg, first, 0x0E) & 0x80 != 0 {
            8
        } else {
            1
        };
        for function in 0..functions {
            let at = Address { function, ..first };
            let Some(f) = (if function == 0 {
                Some(f0)
            } else {
                probe(cfg, at)
            }) else {
                continue;
            };
            found.push(f);
            if f.header_type == 1 && f.class == 0x06 && f.subclass == 0x04 {
                let secondary = read8(cfg, at, 0x19);
                if secondary != 0 {
                    scan_bus(cfg, segment, secondary, visited, found);
                }
            }
        }
    }
}

/// A decoded base address register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    /// Memory space: physical address and size in bytes.
    Memory {
        /// Physical address.
        addr: u64,
        /// Size in bytes.
        size: u64,
        /// Prefetchable.
        prefetchable: bool,
        /// A 64-bit BAR (it also uses the next register).
        wide: bool,
    },
    /// I/O space: port and size.
    Io {
        /// First port.
        port: u32,
        /// Number of ports.
        size: u32,
    },
}

/// Decodes a BAR from its value (`low`, and `high` for the upper half of a
/// 64-bit memory BAR) and the value read back after writing all ones
/// (`mask_low`, `mask_high`). `None` for an unimplemented BAR.
pub fn decode_bar(low: u32, high: u32, mask_low: u32, mask_high: u32) -> Option<Bar> {
    if low & 1 == 1 {
        if mask_low & 0xFFFF_FFFC == 0 {
            return None;
        }
        // I/O BARs may implement only 16 bits; the upper half reads 0.
        let size = (!(mask_low & 0xFFFF_FFFC) & 0xFFFF) + 1;
        return Some(Bar::Io {
            port: low & 0xFFFF_FFFC,
            size,
        });
    }
    let wide = (low >> 1) & 3 == 2;
    let prefetchable = low & 8 != 0;
    let addr_mask = if wide {
        u64::from(mask_high) << 32 | u64::from(mask_low & 0xFFFF_FFF0)
    } else {
        0xFFFF_FFFF_0000_0000 | u64::from(mask_low & 0xFFFF_FFF0)
    };
    if mask_low & 0xFFFF_FFF0 == 0 && (!wide || mask_high == 0) {
        return None;
    }
    let size = (!addr_mask).wrapping_add(1);
    let addr = if wide {
        u64::from(high) << 32 | u64::from(low & 0xFFFF_FFF0)
    } else {
        u64::from(low & 0xFFFF_FFF0)
    };
    Some(Bar::Memory {
        addr,
        size,
        prefetchable,
        wide,
    })
}

/// Reads and sizes BAR `index` (0-5) of a type-0 function. Decoding is
/// switched off while the all-ones pattern is in the register (PCI Local
/// Bus Specification 3.0 section 6.2.5.1), then restored.
pub fn bar(cfg: &dyn ConfigSpace, f: &Function, index: u8) -> Option<Bar> {
    if f.header_type != 0 || index > 5 {
        return None;
    }
    let at = f.address;
    let offset = 0x10 + 4 * u16::from(index);
    let saved_command = read16(cfg, at, 0x04);
    write16(
        cfg,
        at,
        0x04,
        saved_command & !(command::IO_SPACE | command::MEMORY_SPACE),
    );
    let low = cfg.read32(at, offset);
    cfg.write32(at, offset, 0xFFFF_FFFF);
    let mask_low = cfg.read32(at, offset);
    cfg.write32(at, offset, low);
    let wide = low & 1 == 0 && (low >> 1) & 3 == 2 && index < 5;
    let (high, mask_high) = if wide {
        let high = cfg.read32(at, offset + 4);
        cfg.write32(at, offset + 4, 0xFFFF_FFFF);
        let mask = cfg.read32(at, offset + 4);
        cfg.write32(at, offset + 4, high);
        (high, mask)
    } else {
        (0, 0)
    };
    write16(cfg, at, 0x04, saved_command);
    decode_bar(low, high, mask_low, mask_high)
}

/// The offsets of the function's capabilities, with their ids, in list
/// order (at most 48, so a looping list ends).
pub fn capabilities(cfg: &dyn ConfigSpace, at: Address) -> Vec<(u8, u16)> {
    let mut caps = Vec::new();
    // Status bit 4: the capability list exists.
    if read16(cfg, at, 0x06) & 0x10 == 0 {
        return caps;
    }
    let mut next = read8(cfg, at, 0x34) & 0xFC;
    while next >= 0x40 && caps.len() < 48 {
        let offset = u16::from(next);
        caps.push((read8(cfg, at, offset), offset));
        next = read8(cfg, at, offset + 1) & 0xFC;
    }
    caps
}

/// The offset of capability `id`, if the function has it.
pub fn find_capability(cfg: &dyn ConfigSpace, at: Address, id: u8) -> Option<u16> {
    capabilities(cfg, at)
        .into_iter()
        .find(|&(cap, _)| cap == id)
        .map(|(_, offset)| offset)
}

/// The MSI message address for a fixed, physical-destination interrupt
/// to the local APIC `apic_id` (Intel SDM Vol. 3A 11.11.1: 0xFEE in bits
/// 31:20, destination id in 19:12, RH = DM = 0).
pub const fn msi_address(apic_id: u8) -> u32 {
    0xFEE0_0000 | (apic_id as u32) << 12
}

/// The MSI message data for `vector`: edge triggered, fixed delivery mode
/// (Vol. 3A 11.11.2: vector in bits 7:0, delivery mode 10:8 = 0, trigger
/// mode bit 15 = 0).
pub const fn msi_data(vector: u8) -> u16 {
    vector as u16
}

/// Why MSI could not be enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsiError {
    /// The function has no MSI capability.
    NoCapability,
}

/// Programs the MSI capability to send `vector` to `apic_id`, with one
/// message, enables it and disables INTx (PCI Local Bus Specification 3.0
/// section 6.8.1: message control at +2, bit 7 = 64-bit address, bits 6:4
/// multiple message enable, bit 0 enable; address at +4, upper address at
/// +8 if 64-bit, data at +8 or +12).
pub fn enable_msi(
    cfg: &dyn ConfigSpace,
    at: Address,
    vector: u8,
    apic_id: u8,
) -> Result<(), MsiError> {
    let cap = find_capability(cfg, at, capability::MSI).ok_or(MsiError::NoCapability)?;
    let control = read16(cfg, at, cap + 2);
    let wide = control & 0x80 != 0;
    cfg.write32(at, cap + 4, msi_address(apic_id));
    let data_offset = if wide {
        cfg.write32(at, cap + 8, 0);
        cap + 12
    } else {
        cap + 8
    };
    write16(cfg, at, data_offset, msi_data(vector));
    // One message (MME = 0), enabled.
    write16(cfg, at, cap + 2, (control & !0x70) | 1);
    let cmd = read16(cfg, at, 0x04);
    write16(cfg, at, 0x04, cmd | command::INTX_DISABLE);
    Ok(())
}

/// A PCI location `BB:DD.F` or `SSSS:BB:DD.F` (hexadecimal; P13 K11,
/// `DeviceOpen`): bus, device, function.
pub fn parse_location(s: &str) -> Option<(u8, u8, u8)> {
    let s = match s.matches(':').count() {
        2 => s.split_once(':')?.1,
        1 => s,
        _ => return None,
    };
    let (bus, rest) = s.split_once(':')?;
    let (dev, func) = rest.split_once('.')?;
    let bus = u8::from_str_radix(bus, 16).ok()?;
    let dev = u8::from_str_radix(dev, 16).ok()?;
    let func = u8::from_str_radix(func, 16).ok()?;
    (dev < 32 && func < 8).then_some((bus, dev, func))
}

/// Sets bits in the command register.
pub fn enable(cfg: &dyn ConfigSpace, at: Address, bits: u16) {
    let cmd = read16(cfg, at, 0x04);
    write16(cfg, at, 0x04, cmd | bits);
}

#[cfg(test)]
mod tests;
