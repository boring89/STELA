// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The I/O APICs: they receive the interrupt lines of the devices (the
//! global system interrupts, GSIs) and forward each as a message with a
//! vector to a local APIC (Intel 82093AA I/O APIC datasheet, 3.0 "Register
//! Description").
//!
//! Registers are reached indirectly: write the register index to IOREGSEL
//! (offset 0x00), then read or write IOWIN (offset 0x10). Index 0x01 is the
//! version register (bits 16-23: the highest redirection entry), and entry
//! `n` of the redirection table is the pair 0x10 + 2n (low) / 0x11 + 2n
//! (high):
//!
//! ```text
//! low  bits 0-7   vector
//!      bits 8-10  delivery mode (000 = fixed)
//!      bit 11     destination mode (0 = physical APIC id)
//!      bit 13     polarity (1 = active low)
//!      bit 15     trigger (1 = level)
//!      bit 16     mask
//! high bits 24-31 destination APIC id
//! ```

use crate::acpi::tables::{IoApic as MadtIoApic, Madt, Polarity, Trigger};
use crate::mm::mmio::{self, MmioRegion};
use crate::sync::{Once, SpinLock};
use alloc::vec::Vec;

const IOREGSEL: usize = 0x00;
const IOWIN: usize = 0x10;
const VERSION: u32 = 0x01;
const REDIRECTION: u32 = 0x10;

const MASKED: u32 = 1 << 16;
const LEVEL: u32 = 1 << 15;
const ACTIVE_LOW: u32 = 1 << 13;

/// One I/O APIC. The select/window pair is two accesses, so it is locked.
struct IoApic {
    regs: SpinLock<MmioRegion>,
    gsi_base: u32,
    inputs: u32,
}

impl IoApic {
    fn read(&self, register: u32) -> u32 {
        let regs = self.regs.lock();
        regs.write_u32(IOREGSEL, register);
        regs.read_u32(IOWIN)
    }

    fn write(&self, register: u32, value: u32) {
        let regs = self.regs.lock();
        regs.write_u32(IOREGSEL, register);
        regs.write_u32(IOWIN, value);
    }
}

static IO_APICS: Once<Vec<IoApic>> = Once::new();

fn io_apics() -> &'static [IoApic] {
    IO_APICS.get().map_or(&[], Vec::as_slice)
}

/// Maps every I/O APIC of the MADT and masks all their inputs (the
/// firmware may have left some enabled).
pub(crate) fn init(madt: &Madt) {
    let mut list = Vec::new();
    for &MadtIoApic {
        id,
        address,
        gsi_base,
    } in &madt.io_apics
    {
        let regs = match mmio::map(u64::from(address), 0x20) {
            Ok(regs) => regs,
            Err(e) => panic!("ioapic: cannot map I/O APIC {id} at {address:#x}: {e:?}"),
        };
        let mut apic = IoApic {
            regs: SpinLock::new(regs),
            gsi_base,
            inputs: 0,
        };
        apic.inputs = ((apic.read(VERSION) >> 16) & 0xFF) + 1;
        for n in 0..apic.inputs {
            apic.write(REDIRECTION + 2 * n, MASKED);
            apic.write(REDIRECTION + 2 * n + 1, 0);
        }
        crate::kprintln!(
            "ioapic: {} at {:#x}, GSI {}..{}",
            id,
            address,
            gsi_base,
            gsi_base + apic.inputs
        );
        list.push(apic);
    }
    let _ = IO_APICS.set(list);
}

/// The I/O APIC and input number that receive `gsi`.
fn find(gsi: u32) -> Option<(&'static IoApic, u32)> {
    io_apics()
        .iter()
        .find(|a| gsi >= a.gsi_base && gsi - a.gsi_base < a.inputs)
        .map(|a| (a, gsi - a.gsi_base))
}

/// Sends `gsi` as `vector` to the local APIC `apic_id` (fixed delivery,
/// physical destination) and unmasks it. Returns false if no I/O APIC has
/// that input.
pub(crate) fn route(
    gsi: u32,
    vector: u8,
    apic_id: u32,
    polarity: Polarity,
    trigger: Trigger,
) -> bool {
    let Some((apic, input)) = find(gsi) else {
        return false;
    };
    let mut low = u32::from(vector);
    if polarity == Polarity::ActiveLow {
        low |= ACTIVE_LOW;
    }
    if trigger == Trigger::Level {
        low |= LEVEL;
    }
    // Destination first, then the low half, which unmasks.
    apic.write(REDIRECTION + 2 * input + 1, apic_id << 24);
    apic.write(REDIRECTION + 2 * input, low);
    true
}

/// Masks `gsi`. Returns false if no I/O APIC has that input.
pub(crate) fn mask(gsi: u32) -> bool {
    let Some((apic, input)) = find(gsi) else {
        return false;
    };
    let low = apic.read(REDIRECTION + 2 * input);
    apic.write(REDIRECTION + 2 * input, low | MASKED);
    true
}

/// The low half of the redirection entry for `gsi`, for tests.
pub(crate) fn entry(gsi: u32) -> Option<u32> {
    find(gsi).map(|(apic, input)| apic.read(REDIRECTION + 2 * input))
}
