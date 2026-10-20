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

/// The MSI-X table regions mapped so far (kept for good: the mapping
/// must outlive the device's use of it).
static MSIX_TABLES: SpinLock<Vec<MmioRegion>> = SpinLock::new(Vec::new());

/// Allocates a vector, installs `handler` for it and points `f`'s MSI (or,
/// without MSI, entry 0 of its MSI-X table) at this processor. Returns the
/// vector.
pub fn enable_msi(f: &Function, handler: frame::irq::Handler) -> Result<u8, InterruptError> {
    let cfg = config();
    let msi = super::find_capability(cfg, f.address, super::capability::MSI);
    let msix = super::find_capability(cfg, f.address, super::capability::MSI_X);
    if msi.is_none() && msix.is_none() {
        return Err(InterruptError::NoMsi);
    }
    let vector = frame::irq::alloc_vector().ok_or(InterruptError::NoVector)?;
    frame::irq::register(vector, handler).map_err(InterruptError::Register)?;
    // xAPIC ids are 8 bits (Intel SDM Vol. 3A 11.4.6).
    let apic_id = frame::cpu::current().apic_id() as u8;
    let result = match (msi, msix) {
        (Some(_), _) => super::enable_msi(cfg, f.address, vector, apic_id).map_err(|e| match e {
            MsiError::NoCapability => InterruptError::NoMsi,
        }),
        (None, Some(cap)) => enable_msix(f, cap, vector, apic_id),
        (None, None) => Err(InterruptError::NoMsi),
    };
    if result.is_err() {
        frame::irq::unregister(vector);
    }
    result.map(|()| vector)
}

/// Programs entry 0 of the MSI-X table (PCI Local Bus Specification 3.0
/// section 6.8.2: message control at +2, bit 15 enable, bit 14 function
/// mask; table offset and BAR indicator at +4; each entry is address low,
/// address high, data, vector control (bit 0 masked)).
fn enable_msix(f: &Function, cap: u16, vector: u8, apic_id: u8) -> Result<(), InterruptError> {
    let cfg = config();
    let table = cfg.read32(f.address, cap + 4);
    let bir = (table & 7) as u8;
    let Some(Bar::Memory { addr, .. }) = super::bar(cfg, f, bir) else {
        return Err(InterruptError::NoMsi);
    };
    let region = mmio::map(addr + u64::from(table & !7), 16).map_err(|_| InterruptError::NoMsi)?;
    region.write_u32(0, super::msi_address(apic_id));
    region.write_u32(4, 0);
    region.write_u32(8, u32::from(super::msi_data(vector)));
    region.write_u32(12, 0);
    let control = super::read16(cfg, f.address, cap + 2);
    super::write16(cfg, f.address, cap + 2, (control | 1 << 15) & !(1 << 14));
    super::enable(
        cfg,
        f.address,
        super::command::INTX_DISABLE | super::command::MEMORY_SPACE,
    );
    MSIX_TABLES.lock().push(region);
    Ok(())
}
