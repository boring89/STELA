// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host tests over a fake bus.

use super::*;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// A fake configuration space: 256 bytes per function, with writable
/// BARs that read back their size mask after an all-ones write.
#[derive(Default)]
struct FakeBus {
    functions: RefCell<BTreeMap<Address, [u32; 64]>>,
    /// (function, register) -> size mask of that BAR register.
    bar_masks: BTreeMap<(Address, u16), u32>,
}

impl FakeBus {
    fn add(&mut self, at: Address, vendor: u16, device: u16, class: [u8; 3], header: u8) {
        let mut regs = [0u32; 64];
        regs[0] = u32::from(device) << 16 | u32::from(vendor);
        regs[2] = u32::from(class[0]) << 24 | u32::from(class[1]) << 16 | u32::from(class[2]) << 8;
        regs[3] = u32::from(header) << 16;
        self.functions.borrow_mut().insert(at, regs);
    }

    fn set(&self, at: Address, offset: u16, value: u32) {
        self.functions.borrow_mut().get_mut(&at).unwrap()[usize::from(offset / 4)] = value;
    }
}

impl ConfigSpace for FakeBus {
    fn read32(&self, at: Address, offset: u16) -> u32 {
        match self.functions.borrow().get(&at) {
            Some(regs) => regs[usize::from(offset / 4)],
            None => 0xFFFF_FFFF,
        }
    }

    fn write32(&self, at: Address, offset: u16, value: u32) {
        let mut functions = self.functions.borrow_mut();
        let Some(regs) = functions.get_mut(&at) else {
            return;
        };
        let value = match self.bar_masks.get(&(at, offset)) {
            // The address bits that are hardwired to 0 stay 0; the type
            // bits stay as they are.
            Some(&mask) => (value & mask) | (regs[usize::from(offset / 4)] & !mask & 0xF),
            // An unimplemented BAR is hardwired to 0.
            None if (0x10..0x28).contains(&offset) => 0,
            None => value,
        };
        regs[usize::from(offset / 4)] = value;
    }
}

fn addr(bus: u8, device: u8, function: u8) -> Address {
    Address {
        segment: 0,
        bus,
        device,
        function,
    }
}

#[test]
fn scan_finds_functions_and_buses_behind_bridges() {
    let mut bus = FakeBus::default();
    bus.add(addr(0, 0, 0), 0x8086, 0x29C0, [6, 0, 0], 0); // host bridge
    bus.add(addr(0, 2, 0), 0x1234, 0x1111, [3, 0, 0], 0); // display
    bus.add(addr(0, 0x1F, 0), 0x8086, 0x2918, [6, 1, 0], 0x80); // ISA, multi-function
    bus.add(addr(0, 0x1F, 2), 0x8086, 0x2922, [1, 6, 1], 0); // AHCI
    bus.add(addr(0, 0x1F, 3), 0x8086, 0x2930, [0x0C, 5, 0], 0); // SMBus
    bus.add(addr(0, 3, 0), 0x1B36, 0x000C, [6, 4, 0], 1); // PCI bridge
    bus.set(addr(0, 3, 0), 0x18, 0x0002_0100); // secondary 1, subordinate 2
    bus.add(addr(1, 0, 0), 0x1B36, 0x0010, [1, 8, 2], 0); // NVMe behind it
    // Function 1 of a single-function device is not scanned.
    bus.add(addr(0, 2, 1), 0xDEAD, 0xBEEF, [0xFF, 0, 0], 0);
    let found = scan(&bus, 0);
    let addresses: Vec<_> = found.iter().map(|f| f.address).collect();
    assert_eq!(
        addresses,
        [
            addr(0, 0, 0),
            addr(0, 2, 0),
            addr(0, 3, 0),
            addr(0, 0x1F, 0),
            addr(0, 0x1F, 2),
            addr(0, 0x1F, 3),
            addr(1, 0, 0)
        ]
    );
    let ahci = found.iter().find(|f| f.is(1, 6, 1)).unwrap();
    assert_eq!(ahci.kind(), "AHCI controller");
    assert_eq!((ahci.vendor, ahci.device), (0x8086, 0x2922));
    assert_eq!(found[6].kind(), "NVMe controller");
    assert_eq!(addr(1, 0x1F, 7).to_string(), "0000:01:1f.7");
}

#[test]
fn scan_finds_buses_no_bridge_leads_to() {
    // The P520's layout (P12): devices on root buses 0x17 and 0x65 that
    // no bridge on bus 0 points to; a bridge's bus is still scanned once.
    let mut bus = FakeBus::default();
    bus.add(addr(0, 0, 0), 0x8086, 0x2020, [6, 0, 0], 0);
    bus.add(addr(0, 3, 0), 0x8086, 0x2030, [6, 4, 0], 1);
    bus.set(addr(0, 3, 0), 0x18, 0x0002_0200); // secondary 2
    bus.add(addr(2, 0, 0), 0x8086, 0x15B7, [2, 0, 0], 0);
    bus.add(addr(0x17, 0, 0), 0x2646, 0x5013, [1, 8, 2], 0); // NVMe
    bus.add(addr(0x65, 0, 0), 0x10DE, 0x1B81, [3, 0, 0], 0); // GPU
    let found = scan(&bus, 0);
    let addresses: Vec<_> = found.iter().map(|f| f.address).collect();
    assert_eq!(
        addresses,
        [
            addr(0, 0, 0),
            addr(0, 3, 0),
            addr(2, 0, 0),
            addr(0x17, 0, 0),
            addr(0x65, 0, 0)
        ]
    );
}

#[test]
fn bars_are_sized() {
    let mut bus = FakeBus::default();
    let at = addr(0, 4, 0);
    bus.add(at, 0x8086, 0x1234, [1, 6, 1], 0);
    // BAR0: 32-bit memory at 0xFEBF_1000, 4 KiB.
    bus.bar_masks.insert((at, 0x10), 0xFFFF_F000);
    bus.set(at, 0x10, 0xFEBF_1000);
    // BAR1: I/O at 0xC040, 32 ports.
    bus.bar_masks.insert((at, 0x14), 0xFFFF_FFE0);
    bus.set(at, 0x14, 0xC041);
    // BAR2-3: 64-bit prefetchable memory at 0x8_0000_0000, 16 KiB.
    bus.bar_masks.insert((at, 0x18), 0xFFFF_C000);
    bus.bar_masks.insert((at, 0x1C), 0xFFFF_FFFF);
    bus.set(at, 0x18, 0x0000_000C);
    bus.set(at, 0x1C, 0x0000_0008);
    bus.set(
        at,
        0x04,
        u32::from(command::MEMORY_SPACE | command::IO_SPACE),
    );
    let f = probe(&bus, at).unwrap();
    assert_eq!(
        bar(&bus, &f, 0),
        Some(Bar::Memory {
            addr: 0xFEBF_1000,
            size: 4096,
            prefetchable: false,
            wide: false
        })
    );
    assert_eq!(
        bar(&bus, &f, 1),
        Some(Bar::Io {
            port: 0xC040,
            size: 32
        })
    );
    assert_eq!(
        bar(&bus, &f, 2),
        Some(Bar::Memory {
            addr: 0x8_0000_0000,
            size: 16384,
            prefetchable: true,
            wide: true
        })
    );
    // Unimplemented BAR.
    assert_eq!(bar(&bus, &f, 4), None);
    // The values and the command register are restored.
    assert_eq!(bus.read32(at, 0x10), 0xFEBF_1000);
    assert_eq!(bus.read32(at, 0x1C), 8);
    assert_eq!(
        read16(&bus, at, 0x04),
        command::MEMORY_SPACE | command::IO_SPACE
    );
}

#[test]
fn capabilities_and_msi() {
    let mut bus = FakeBus::default();
    let at = addr(0, 5, 0);
    bus.add(at, 0x1B36, 0x0010, [1, 8, 2], 0);
    bus.set(at, 0x04, 0x0010_0000); // status: capability list
    bus.set(at, 0x34, 0x40);
    bus.set(at, 0x40, 0x0000_5011); // MSI-X, next 0x50
    bus.set(at, 0x50, 0x0080_6005); // MSI, 64-bit capable, next 0x60
    bus.set(at, 0x60, 0x0000_4010); // PCIe, next 0x40: a loop
    let caps = capabilities(&bus, at);
    assert_eq!(caps.len(), 48);
    assert_eq!(caps[..3], [(0x11, 0x40), (0x05, 0x50), (0x10, 0x60)]);
    assert_eq!(find_capability(&bus, at, capability::MSI), Some(0x50));
    assert_eq!(find_capability(&bus, at, 0x09), None);
    enable_msi(&bus, at, 0x32, 3).unwrap();
    assert_eq!(bus.read32(at, 0x54), 0xFEE0_3000);
    assert_eq!(bus.read32(at, 0x58), 0);
    assert_eq!(read16(&bus, at, 0x5C), 0x32);
    assert_eq!(read16(&bus, at, 0x52) & 0x71, 1);
    assert_ne!(read16(&bus, at, 0x04) & command::INTX_DISABLE, 0);
    // No capabilities at all.
    let other = addr(0, 6, 0);
    bus.add(other, 1, 2, [2, 0, 0], 0);
    assert!(capabilities(&bus, other).is_empty());
    assert_eq!(
        enable_msi(&bus, other, 0x33, 0),
        Err(MsiError::NoCapability)
    );
    assert_eq!(msi_address(0), 0xFEE0_0000);
    assert_eq!(msi_data(0x40), 0x40);
}
