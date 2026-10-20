// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The machine's PCI: configuration access through ECAM (the memory
//! window the ACPI MCFG describes, PCI Express Base Specification 7.2.2) or,
//! without an MCFG, the legacy ports; the device list found at boot; BAR
//! mapping and MSI setup for drivers.
//!
//! ECAM gives each bus 1 MiB (32 devices x 8 functions x 4 KiB). A bus's
//! window is mapped (uncached) the first time it is touched.

use super::{Address, Bar, ConfigSpace, Function, MsiError};
use alloc::collections::BTreeMap;
use alloc::collections::btree_map::Entry;
use alloc::vec::Vec;
use frame::acpi::tables::McfgEntry;
use frame::mm::mmio::{self, MmioRegion};
use frame::sync::{Once, SpinLock};

enum Access {
    Ecam {
        window: McfgEntry,
        buses: SpinLock<BTreeMap<u8, MmioRegion>>,
    },
    Legacy,
}

impl Access {
    /// Calls `f` with the ECAM region of `bus` (mapping it first).
    fn with_bus<R>(&self, bus: u8, f: impl FnOnce(&MmioRegion) -> R) -> Option<R> {
        let Access::Ecam { window, buses } = self else {
            return None;
        };
        if bus < window.start_bus || bus > window.end_bus {
            return None;
        }
        let mut buses = buses.lock();
        let region = match buses.entry(bus) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let phys = window.base + (u64::from(bus - window.start_bus) << 20);
                e.insert(mmio::map(phys, 1 << 20).ok()?)
            }
        };
        Some(f(region))
    }
}

fn ecam_offset(at: Address, offset: u16) -> usize {
    usize::from(at.device & 0x1F) << 15 | usize::from(at.function & 7) << 12 | usize::from(offset)
}

impl ConfigSpace for Access {
    fn read32(&self, at: Address, offset: u16) -> u32 {
        let offset = offset & 0xFFC;
        match self {
            Access::Ecam { .. } => self
                .with_bus(at.bus, |r| r.read_u32(ecam_offset(at, offset)))
                .unwrap_or(0xFFFF_FFFF),
            Access::Legacy if offset < 256 => {
                frame::pci::legacy_read32(at.bus, at.device, at.function, offset as u8)
            }
            Access::Legacy => 0xFFFF_FFFF,
        }
    }

    fn write32(&self, at: Address, offset: u16, value: u32) {
        let offset = offset & 0xFFC;
        match self {
            Access::Ecam { .. } => {
                self.with_bus(at.bus, |r| r.write_u32(ecam_offset(at, offset), value));
            }
            Access::Legacy if offset < 256 => {
                frame::pci::legacy_write32(at.bus, at.device, at.function, offset as u8, value)
            }
            Access::Legacy => {}
        }
    }
}

static ACCESS: Once<Access> = Once::new();
static DEVICES: Once<Vec<Function>> = Once::new();

/// Chooses the configuration access, scans segment 0 and prints what it
/// found. Call once at boot, after `frame::init`.
pub fn init() {
    let access = match frame::acpi::mcfg()
        .iter()
        .find(|w| w.segment == 0 && w.start_bus == 0)
    {
        Some(&window) => Access::Ecam {
            window,
            buses: SpinLock::new(BTreeMap::new()),
        },
        None => Access::Legacy,
    };
    let how = match &access {
        Access::Ecam { window, .. } => alloc::format!(
            "ECAM at {:#x}, buses {}-{}",
            window.base,
            window.start_bus,
            window.end_bus
        ),
        Access::Legacy => alloc::string::String::from("legacy ports 0xCF8/0xCFC (no MCFG)"),
    };
    let _ = ACCESS.set(access);
    let found = super::scan(config(), 0);
    frame::kprintln!("pci: {how}, {} function(s)", found.len());
    for f in &found {
        frame::kprintln!(
            "pci: {} {:04x}:{:04x} class {:02x}/{:02x}/{:02x} {}",
            f.address,
            f.vendor,
            f.device,
            f.class,
            f.subclass,
            f.prog_if,
            f.kind()
        );
    }
    let _ = DEVICES.set(found);
}

/// The configuration access (legacy ports before [`init`]).
pub fn config() -> &'static dyn ConfigSpace {
    match ACCESS.get() {
        Some(access) => access,
        None => &Access::Legacy,
    }
}

/// Whether configuration space goes through ECAM.
pub fn uses_ecam() -> bool {
    matches!(ACCESS.get(), Some(Access::Ecam { .. }))
}

/// The functions found at boot.
pub fn devices() -> &'static [Function] {
    DEVICES.get().map_or(&[], Vec::as_slice)
}

/// Why a BAR could not be mapped.
#[derive(Debug)]
pub enum BarError {
    /// The BAR is missing or is an I/O BAR.
    NotMemory,
    /// The mapping failed.
    Map(mmio::MmioError),
}

/// Maps memory BAR `index` of `f` (uncached) and enables memory decoding.
pub fn map_bar(f: &Function, index: u8) -> Result<MmioRegion, BarError> {
    let Some(Bar::Memory { addr, size, .. }) = super::bar(config(), f, index) else {
        return Err(BarError::NotMemory);
    };
    let size = usize::try_from(size).map_err(|_| BarError::NotMemory)?;
    let region = mmio::map(addr, size).map_err(BarError::Map)?;
    super::enable(config(), f.address, super::command::MEMORY_SPACE);
    Ok(region)
}

/// Lets `f` master the bus (DMA, MSI writes).
pub fn enable_bus_master(f: &Function) {
    super::enable(config(), f.address, super::command::BUS_MASTER);
}

/// Why MSI could not be set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptError {
    /// No MSI capability (MSI-X and INTx are not supported yet).
    NoMsi,
    /// No vector left.
    NoVector,
    /// The handler could not be installed.
    Register(frame::irq::IrqError),
}

/// Allocates a vector, installs `handler` for it and points `f`'s MSI at
/// this processor. Returns the vector.
pub fn enable_msi(f: &Function, handler: frame::irq::Handler) -> Result<u8, InterruptError> {
    super::find_capability(config(), f.address, super::capability::MSI)
        .ok_or(InterruptError::NoMsi)?;
    let vector = frame::irq::alloc_vector().ok_or(InterruptError::NoVector)?;
    frame::irq::register(vector, handler).map_err(InterruptError::Register)?;
    // xAPIC ids are 8 bits (Intel SDM Vol. 3A 11.4.6).
    let apic_id = frame::cpu::current().apic_id() as u8;
    match super::enable_msi(config(), f.address, vector, apic_id) {
        Ok(()) => Ok(vector),
        Err(MsiError::NoCapability) => {
            frame::irq::unregister(vector);
            Err(InterruptError::NoMsi)
        }
    }
}
