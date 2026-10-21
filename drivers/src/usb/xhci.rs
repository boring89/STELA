// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The xHCI host controller (eXtensible Host Controller Interface for USB
//! 1.2; `docs/usb.md`): one controller, root ports and USB 2.0 hubs, HID
//! boot keyboards and mice, hot plugging.
//!
//! ```text
//!  BAR0: capability regs | operational regs (USBCMD, USBSTS, CRCR, DCBAAP,
//!        CONFIG, PORTSC...) | runtime regs (interrupter 0) | doorbells
//!  DMA:  DCBAA (device context pointers) · command ring · event ring + ERST
//!        per device: output context · input context · EP0 ring · buffers
//! ```
//!
//! Everything runs on one kernel thread ([`run`]): it waits for the MSI
//! (or 50 ms), drains the event ring, re-queues HID reports into the
//! input queue, and services ports that changed. Commands and control
//! transfers issued during enumeration drain the event ring themselves
//! while they wait, so reports keep flowing.

use super::hid::{self, Keyboard};
use super::trb::{Consumer, Producer, Trb, completion, kind};
use super::{
    Configuration, Setup, Speed, configuration_length, descriptor, interrupt_interval,
    parse_configuration, parse_device, parse_port_status, port_feature,
};
use crate::pci::Function;
use crate::pci::system as pci;
use crate::sync::{wait_for, wake};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use frame::input::{self, InputEvent};
use frame::mm::dma::DmaBuffer;
use frame::mm::mmio::MmioRegion;
use frame::sync::SpinLock;
use frame::trap::TrapFrame;

// Capability registers (xHCI 1.2 5.3).
const CAPLENGTH: usize = 0x00;
const HCSPARAMS1: usize = 0x04;
const HCSPARAMS2: usize = 0x08;
const HCCPARAMS1: usize = 0x10;
const DBOFF: usize = 0x14;
const RTSOFF: usize = 0x18;

// Operational registers (5.4), from the operational base.
const USBCMD: usize = 0x00;
const USBSTS: usize = 0x04;
const CRCR: usize = 0x18;
const DCBAAP: usize = 0x30;
const CONFIG: usize = 0x38;
const PORTSC: usize = 0x400;
const CMD_RUN: u32 = 1 << 0;
const CMD_RESET: u32 = 1 << 1;
const CMD_INTE: u32 = 1 << 2;
const STS_HALTED: u32 = 1 << 0;
const STS_EINT: u32 = 1 << 3;
const STS_CNR: u32 = 1 << 11;

// PORTSC bits (5.4.8).
const PORT_CCS: u32 = 1 << 0;
const PORT_PED: u32 = 1 << 1;
const PORT_PR: u32 = 1 << 4;
const PORT_PP: u32 = 1 << 9;
const PORT_CSC: u32 = 1 << 17;
const PORT_PRC: u32 = 1 << 21;
/// The change bits (17-23), all write-1-to-clear.
const PORT_CHANGES: u32 = 0x00FE_0000;
/// Bits that keep their value when written back: read-only and
/// read/write (not the write-1-to-clear PED and change bits, not PR).
const PORT_KEEP: u32 = 0x4E00_FFE9 & !PORT_PED;

// Runtime registers (5.5): interrupter 0.
const IMAN: usize = 0x20;
const IMOD: usize = 0x24;
const ERSTSZ: usize = 0x28;
const ERSTBA: usize = 0x30;
const ERDP: usize = 0x38;
const IMAN_IP: u32 = 1 << 0;
const IMAN_IE: u32 = 1 << 1;
/// ERDP bit 3: event handler busy (write 1 to clear).
const ERDP_EHB: u64 = 1 << 3;

/// TRBs per ring page.
const RING_TRBS: usize = 256;
/// Longest wait for a command or a control transfer.
const TIMEOUT_NS: u64 = 1_000_000_000;
/// How often hubs' ports are polled.
const HUB_POLL_NS: u64 = 300_000_000;
/// Deepest hub chain (USB 2.0 4.1.1: five hubs).
const MAX_DEPTH: u8 = 5;

/// Why a USB operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbError {
    /// No answer in time.
    Timeout,
    /// The controller reported this completion code.
    Completion(u8),
    /// Out of DMA memory or slots.
    NoMemory,
    /// A descriptor did not make sense.
    BadDescriptor,
}

/// A TRB ring in one DMA page (the last TRB links back to the first).
struct Ring {
    page: DmaBuffer,
    producer: Producer,
}

impl Ring {
    fn new() -> Result<Ring, UsbError> {
        let page = DmaBuffer::new(1).ok_or(UsbError::NoMemory)?;
        // The Link TRB, not yet handed over (cycle 0).
        let link = Trb::link(page.phys()).with_cycle(false);
        let _ = page.write((RING_TRBS - 1) * 16, &link.to_bytes());
        Ok(Ring {
            page,
            producer: Producer::new(RING_TRBS),
        })
    }

    fn phys(&self) -> u64 {
        self.page.phys()
    }

    /// Queues `trb`; returns its physical address.
    fn push(&mut self, trb: Trb) -> u64 {
        let (at, cycle, link) = self.producer.advance();
        let _ = self.page.write(at * 16, &trb.with_cycle(cycle).to_bytes());
        if let Some(cycle) = link {
            let link = Trb::link(self.page.phys()).with_cycle(cycle);
            let _ = self.page.write((RING_TRBS - 1) * 16, &link.to_bytes());
        }
        self.page.phys() + at as u64 * 16
    }
}

/// What a device is to STELA.
enum Role {
    /// Nothing STELA drives.
    Other,
    /// A HID boot keyboard or mouse with its interrupt IN endpoint.
    Hid {
        mouse: bool,
        dci: u8,
        ring: Ring,
        report: DmaBuffer,
        length: u16,
        keyboard: Keyboard,
        /// A report has arrived (the first one is logged).
        seen: bool,
    },
    /// A hub with `ports` ports; `children[p - 1]` is the slot on port p.
    Hub {
        ports: u8,
        children: Vec<Option<u8>>,
    },
    /// Mass storage, Bulk-Only Transport (P11 K41, `storage`).
    Storage {
        out_dci: u8,
        in_dci: u8,
        out_ring: Ring,
        in_ring: Ring,
        /// The CBW and CSW.
        wrapper: DmaBuffer,
        /// The data stage (4 KiB).
        data: DmaBuffer,
        tag: u32,
        /// Bytes of a sector (512 or 4096).
        sector: u32,
        disk: Arc<super::storage::UsbDisk>,
        name: String,
    },
}

/// What the kernel does when a disk comes and goes (P11 K41).
#[derive(Clone, Copy)]
pub struct StorageHooks {
    /// A disk arrived: its name (`usb1`...) and the device.
    pub attach: fn(String, Arc<dyn block::BlockDevice + Send + Sync>),
    /// The disk of that name went away.
    pub detach: fn(&str),
}

static STORAGE_HOOKS: SpinLock<Option<StorageHooks>> = SpinLock::new(None);
static NEXT_DISK: AtomicU64 = AtomicU64::new(1);

/// Sets what happens when a USB disk comes and goes.
pub fn set_storage_hooks(hooks: StorageHooks) {
    *STORAGE_HOOKS.lock() = Some(hooks);
}

/// Wakes the USB thread (a disk request is waiting).
pub fn kick() {
    wake(WAITER.load(Ordering::Acquire));
}

/// One device in a slot.
struct Device {
    path: String,
    speed: Speed,
    root_port: u8,
    route: u32,
    depth: u8,
    /// (hub slot, port) the device hangs off, if not a root port.
    parent: Option<(u8, u8)>,
    output: DmaBuffer,
    input: DmaBuffer,
    ep0: Ring,
    buffer: DmaBuffer,
    max_packet0: u16,
    role: Role,
    /// Vendor and product ids from the device descriptor.
    ids: (u16, u16),
}

/// A device as the rest of the kernel sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Where it is: the root port, then hub ports (`3`, `3.1`).
    pub path: String,
    /// `keyboard`, `mouse`, `hub` or `other`.
    pub kind: &'static str,
    /// Its xHCI slot.
    pub slot: u8,
    /// Vendor and product ids (P11, for the hardware report).
    pub ids: (u16, u16),
    /// `low`, `full`, `high` or `super`.
    pub speed: &'static str,
}

static DEVICES: SpinLock<Vec<DeviceInfo>> = SpinLock::new(Vec::new());

/// The USB devices attached now.
pub fn devices() -> Vec<DeviceInfo> {
    DEVICES.lock().clone()
}

/// The controller's registers and the thread to wake, for the interrupt.
static IRQ: SpinLock<Option<(Arc<MmioRegion>, usize)>> = SpinLock::new(None);
static WAITER: AtomicU64 = AtomicU64::new(0);

fn interrupt(_: &TrapFrame) {
    if let Some((regs, rt)) = IRQ.lock().as_ref() {
        regs.write_u32(rt + IMAN, IMAN_IP | IMAN_IE);
    }
    wake(WAITER.load(Ordering::Acquire));
}

/// A started controller.
pub struct Controller {
    regs: Arc<MmioRegion>,
    op: usize,
    rt: usize,
    db: usize,
    ports: u8,
    ctx: usize,
    dcbaa: DmaBuffer,
    _scratch: Vec<DmaBuffer>,
    commands: Ring,
    events: DmaBuffer,
    _erst: DmaBuffer,
    consumer: Consumer,
    devices: BTreeMap<u8, Device>,
    root: Vec<Option<u8>>,
    changed_ports: u64,
    name: String,
}

fn spin(ns: u64) {
    if let Some(h) = crate::sync::hooks() {
        let until = (h.now_ns)() + ns;
        while (h.now_ns)() < until {
            (h.block_until)(Some(until));
        }
    }
}

impl Controller {
    fn op_read(&self, reg: usize) -> u32 {
        self.regs.read_u32(self.op + reg)
    }

    fn op_write(&self, reg: usize, v: u32) {
        self.regs.write_u32(self.op + reg, v)
    }

    fn op_write64(&self, reg: usize, v: u64) {
        self.op_write(reg, v as u32);
        self.op_write(reg + 4, (v >> 32) as u32);
    }

    fn rt_write64(&self, reg: usize, v: u64) {
        self.regs.write_u32(self.rt + reg, v as u32);
        self.regs.write_u32(self.rt + reg + 4, (v >> 32) as u32);
    }

    fn portsc(&self, port: u8) -> u32 {
        self.op_read(PORTSC + 0x10 * (usize::from(port) - 1))
    }

    fn set_portsc(&self, port: u8, v: u32) {
        self.op_write(PORTSC + 0x10 * (usize::from(port) - 1), v)
    }

    fn ring_doorbell(&self, slot: u8, target: u8) {
        self.regs
            .write_u32(self.db + 4 * usize::from(slot), u32::from(target));
    }

    // ---- events ----

    fn peek_event(&self) -> Option<Trb> {
        let mut b = [0u8; 16];
        self.events.read(self.consumer.index * 16, &mut b).ok()?;
        let trb = Trb::from_bytes(&b);
        self.consumer.is_new(&trb).then_some(trb)
    }

    fn next_event(&mut self) -> Option<Trb> {
        let trb = self.peek_event()?;
        self.consumer.advance();
        let at = self.events.phys() + self.consumer.index as u64 * 16;
        self.rt_write64(ERDP, at | ERDP_EHB);
        Some(trb)
    }

    /// Handles an event nobody is waiting for: HID reports, port changes.
    fn dispatch(&mut self, e: Trb) {
        match e.kind() {
            kind::PORT_STATUS_CHANGE => {
                let port = e.port();
                if port < 64 {
                    self.changed_ports |= 1 << port;
                }
            }
            kind::TRANSFER_EVENT => self.hid_event(e),
            _ => {}
        }
    }

    /// Waits for the event `wanted` picks, handling the others; at most
    /// `TIMEOUT_NS`.
    fn wait_event(&mut self, wanted: impl Fn(&Trb) -> bool) -> Result<Trb, UsbError> {
        let deadline = crate::sync::hooks().map_or(0, |h| (h.now_ns)()) + TIMEOUT_NS;
        loop {
            while let Some(e) = self.next_event() {
                if wanted(&e) {
                    return Ok(e);
                }
                self.dispatch(e);
            }
            let now = crate::sync::hooks().map_or(deadline, |h| (h.now_ns)());
            if now >= deadline {
                return Err(UsbError::Timeout);
            }
            wait_for(
                deadline - now,
                |me| WAITER.store(me, Ordering::Release),
                || self.peek_event().is_some(),
            );
        }
    }

    fn command(&mut self, trb: Trb) -> Result<Trb, UsbError> {
        let at = self.commands.push(trb);
        self.ring_doorbell(0, 0);
        let e = self.wait_event(|e| e.kind() == kind::COMMAND_COMPLETION && e.parameter == at)?;
        match e.completion() {
            completion::SUCCESS => Ok(e),
            c => Err(UsbError::Completion(c)),
        }
    }

    /// A control transfer on `slot`'s endpoint 0: `data` is filled (IN) or
    /// sent (OUT, `out`); returns the bytes moved.
    fn control(&mut self, slot: u8, setup: Setup, data: &mut [u8]) -> Result<usize, UsbError> {
        let data_in = setup.request_type & super::DIR_IN != 0;
        let length = setup.length.min(4096) as usize;
        let status_at = {
            let dev = self.devices.get_mut(&slot).ok_or(UsbError::BadDescriptor)?;
            if !data_in && length > 0 {
                let _ = dev.buffer.write(0, &data[..length]);
            }
            dev.ep0
                .push(Trb::setup(setup.to_u64(), length as u16, data_in));
            if length > 0 {
                let buf = dev.buffer.phys();
                dev.ep0.push(Trb::data(buf, length as u16, data_in));
            }
            dev.ep0.push(Trb::status(data_in, length > 0))
        };
        self.ring_doorbell(slot, 1);
        let e = self.wait_event(|e| {
            e.kind() == kind::TRANSFER_EVENT
                && e.slot() == slot
                && e.endpoint() == 1
                && e.parameter == status_at
        });
        // A short data stage ends the TD with an event for the data TRB.
        let e = match e {
            Ok(e) => e,
            Err(UsbError::Timeout) => return Err(UsbError::Timeout),
            Err(other) => return Err(other),
        };
        match e.completion() {
            completion::SUCCESS | completion::SHORT_PACKET => {}
            c => return Err(UsbError::Completion(c)),
        }
        if data_in && length > 0 {
            let dev = self.devices.get(&slot).ok_or(UsbError::BadDescriptor)?;
            let _ = dev.buffer.read(0, &mut data[..length]);
        }
        Ok(length)
    }

    // ---- contexts ----

    fn write_ctx(buf: &DmaBuffer, ctx: usize, index: usize, dword: usize, value: u32) {
        let _ = buf.write_u32(index * ctx + dword * 4, value);
    }

    /// Fills the input context for Address Device: slot and EP0.
    fn address_input(&self, dev: &Device, slot_dw2: u32) {
        let c = self.ctx;
        let input = &dev.input;
        input.zero();
        Self::write_ctx(input, c, 0, 1, 0b11);
        Self::write_ctx(
            input,
            c,
            1,
            0,
            dev.route | (dev.speed as u32) << 20 | 1 << 27,
        );
        Self::write_ctx(input, c, 1, 1, u32::from(dev.root_port) << 16);
        Self::write_ctx(input, c, 1, 2, slot_dw2);
        self.ep0_input(dev);
    }

    fn ep0_input(&self, dev: &Device) {
        let c = self.ctx;
        let ring = dev.ep0.phys();
        Self::write_ctx(
            &dev.input,
            c,
            2,
            1,
            3 << 1 | 4 << 3 | u32::from(dev.max_packet0) << 16,
        );
        Self::write_ctx(&dev.input, c, 2, 2, ring as u32 | 1);
        Self::write_ctx(&dev.input, c, 2, 3, (ring >> 32) as u32);
        Self::write_ctx(&dev.input, c, 2, 4, 8);
    }

    // ---- devices ----

    fn publish(&self) {
        let list = self
            .devices
            .iter()
            .map(|(&slot, d)| DeviceInfo {
                path: d.path.clone(),
                slot,
                ids: d.ids,
                speed: d.speed.name(),
                kind: match d.role {
                    Role::Hid { mouse: true, .. } => "mouse",
                    Role::Hid { mouse: false, .. } => "keyboard",
                    Role::Hub { .. } => "hub",
                    Role::Storage { .. } => "storage",
                    Role::Other => "other",
                },
            })
            .collect();
        *DEVICES.lock() = list;
    }

    /// Enumerates the device just reset on root port `root_port` (or on a
    /// hub port `parent`): slot, address, descriptors, configuration.
    fn enumerate(
        &mut self,
        path: String,
        speed: Speed,
        root_port: u8,
        route: u32,
        depth: u8,
        parent: Option<(u8, u8)>,
    ) -> Result<u8, UsbError> {
        let e = self.command(Trb::command(kind::ENABLE_SLOT, 0, 0))?;
        let slot = e.slot();
        let alloc = || DmaBuffer::new(1).ok_or(UsbError::NoMemory);
        let dev = Device {
            path: path.clone(),
            speed,
            root_port,
            route,
            depth,
            parent,
            output: alloc()?,
            input: alloc()?,
            ep0: Ring::new()?,
            buffer: alloc()?,
            max_packet0: speed.default_max_packet0(),
            role: Role::Other,
            ids: (0, 0),
        };
        // A low/full-speed device behind a high-speed hub talks through
        // that hub's transaction translator (slot context dword 2).
        let tt = match parent {
            Some((hub, port))
                if matches!(speed, Speed::Low | Speed::Full)
                    && self
                        .devices
                        .get(&hub)
                        .is_some_and(|h| h.speed == Speed::High) =>
            {
                u32::from(hub) | u32::from(port) << 8
            }
            _ => 0,
        };
        let _ = self
            .dcbaa
            .write_u64(usize::from(slot) * 8, dev.output.phys());
        self.address_input(&dev, tt);
        let input = dev.input.phys();
        self.devices.insert(slot, dev);
        let result = self.setup_device(slot, input);
        if let Err(e) = result {
            frame::kprintln!(
                "usb: {path}: {speed} speed device not set up: {e:?}",
                speed = speed.name()
            );
            self.disable(slot);
            return Err(e);
        }
        self.publish();
        Ok(slot)
    }

    fn setup_device(&mut self, slot: u8, input: u64) -> Result<(), UsbError> {
        self.command(Trb::command(kind::ADDRESS_DEVICE, slot, input))?;
        // The real packet size of endpoint 0.
        let mut head = [0u8; 8];
        self.control(
            slot,
            Setup::get_descriptor(descriptor::DEVICE, 0, 8),
            &mut head,
        )?;
        let speed = self.devices.get(&slot).map_or(Speed::Full, |d| d.speed);
        let mp = if speed == Speed::Super {
            1u16 << head[7].min(15)
        } else {
            u16::from(head[7])
        };
        if mp != 0 && self.devices.get(&slot).is_some_and(|d| d.max_packet0 != mp) {
            let dev = self.devices.get_mut(&slot).ok_or(UsbError::BadDescriptor)?;
            dev.max_packet0 = mp;
            let c = self.ctx;
            dev.input.zero();
            Self::write_ctx(&dev.input, c, 0, 1, 0b10);
            let input = dev.input.phys();
            // EP0 only.
            let dev = self.devices.get(&slot).ok_or(UsbError::BadDescriptor)?;
            self.ep0_input(dev);
            self.command(Trb::command(kind::EVALUATE_CONTEXT, slot, input))?;
        }
        let mut d = [0u8; 18];
        self.control(
            slot,
            Setup::get_descriptor(descriptor::DEVICE, 0, 18),
            &mut d,
        )?;
        let device = parse_device(&d).ok_or(UsbError::BadDescriptor)?;
        if let Some(d) = self.devices.get_mut(&slot) {
            d.ids = (device.vendor, device.product);
        }
        let mut c9 = [0u8; 9];
        self.control(
            slot,
            Setup::get_descriptor(descriptor::CONFIGURATION, 0, 9),
            &mut c9,
        )?;
        let total = configuration_length(&c9)
            .ok_or(UsbError::BadDescriptor)?
            .min(1024);
        let mut all = vec![0u8; usize::from(total)];
        self.control(
            slot,
            Setup::get_descriptor(descriptor::CONFIGURATION, 0, total),
            &mut all,
        )?;
        let config = parse_configuration(&all).ok_or(UsbError::BadDescriptor)?;
        self.control(slot, Setup::set_configuration(config.value), &mut [])?;
        let path = self
            .devices
            .get(&slot)
            .map(|d| d.path.clone())
            .unwrap_or_default();
        let ids = format!("{:04x}:{:04x}", device.vendor, device.product);
        if device.class == 9 || config.interfaces.iter().any(|i| i.class == 9) {
            self.setup_hub(slot)?;
            frame::kprintln!("usb: {path}: hub ({ids}, {} speed)", speed.name());
        } else if let Some(kind) = self.setup_hid(slot, &config)? {
            frame::kprintln!("usb: {path}: {kind} ({ids}, {} speed)", speed.name());
        } else if let Some(what) = self.setup_storage(slot, &config)? {
            frame::kprintln!(
                "usb: {path}: storage {what} ({ids}, {} speed)",
                speed.name()
            );
            self.publish();
        } else {
            frame::kprintln!(
                "usb: {path}: device {ids} (class {}) not used",
                device.class
            );
        }
        Ok(())
    }

    /// Sets up a HID boot keyboard or mouse; `None` if there is none.
    fn setup_hid(
        &mut self,
        slot: u8,
        config: &Configuration,
    ) -> Result<Option<&'static str>, UsbError> {
        let Some(iface) = config
            .interfaces
            .iter()
            .find(|i| i.class == 3 && i.subclass == 1 && (i.protocol == 1 || i.protocol == 2))
        else {
            return Ok(None);
        };
        let Some(ep) = iface
            .endpoints
            .iter()
            .find(|e| e.is_in() && e.kind == 3)
            .copied()
        else {
            return Ok(None);
        };
        let mouse = iface.protocol == 2;
        // Boot protocol, reports only on change; some devices refuse
        // SET_IDLE, which does not matter.
        self.control(slot, Setup::set_protocol(iface.number, 0), &mut [])?;
        let _ = self.control(slot, Setup::set_idle(iface.number), &mut []);
        let ring = Ring::new()?;
        let report = DmaBuffer::new(1).ok_or(UsbError::NoMemory)?;
        let speed = self.devices.get(&slot).map_or(Speed::Full, |d| d.speed);
        let dci = ep.dci();
        let c = self.ctx;
        let input_phys = {
            let dev = self.devices.get(&slot).ok_or(UsbError::BadDescriptor)?;
            let input = &dev.input;
            input.zero();
            Self::write_ctx(input, c, 0, 1, 1 | 1 << dci);
            Self::write_ctx(
                input,
                c,
                1,
                0,
                dev.route | (dev.speed as u32) << 20 | u32::from(dci) << 27,
            );
            Self::write_ctx(input, c, 1, 1, u32::from(dev.root_port) << 16);
            let at = usize::from(dci) + 1;
            let mp = u32::from(ep.max_packet);
            Self::write_ctx(
                input,
                c,
                at,
                0,
                u32::from(interrupt_interval(speed, ep.interval)) << 16,
            );
            Self::write_ctx(input, c, at, 1, 3 << 1 | 7 << 3 | mp << 16);
            Self::write_ctx(input, c, at, 2, ring.phys() as u32 | 1);
            Self::write_ctx(input, c, at, 3, (ring.phys() >> 32) as u32);
            Self::write_ctx(input, c, at, 4, mp | mp << 16);
            input.phys()
        };
        self.command(Trb::command(kind::CONFIGURE_ENDPOINT, slot, input_phys))?;
        let length = ep.max_packet.clamp(1, 64);
        if let Some(dev) = self.devices.get_mut(&slot) {
            dev.role = Role::Hid {
                mouse,
                dci,
                ring,
                report,
                length,
                keyboard: Keyboard::new(),
                seen: false,
            };
        }
        self.queue_report(slot);
        Ok(Some(if mouse { "mouse" } else { "keyboard" }))
    }

    /// Sets up a Bulk-Only mass storage device (class 08, SCSI 06, BOT
    /// 50): both bulk endpoints, then INQUIRY, TEST UNIT READY and READ
    /// CAPACITY; tells the kernel. `None` if it is not one.
    fn setup_storage(
        &mut self,
        slot: u8,
        config: &Configuration,
    ) -> Result<Option<String>, UsbError> {
        let Some(iface) = config
            .interfaces
            .iter()
            .find(|i| i.class == 8 && i.subclass == 6 && i.protocol == 0x50)
        else {
            return Ok(None);
        };
        let (Some(ep_out), Some(ep_in)) = (
            iface
                .endpoints
                .iter()
                .find(|e| !e.is_in() && e.kind == 2)
                .copied(),
            iface
                .endpoints
                .iter()
                .find(|e| e.is_in() && e.kind == 2)
                .copied(),
        ) else {
            return Ok(None);
        };
        let out_ring = Ring::new()?;
        let in_ring = Ring::new()?;
        let wrapper = DmaBuffer::new(1).ok_or(UsbError::NoMemory)?;
        let data = DmaBuffer::new(1).ok_or(UsbError::NoMemory)?;
        let (out_dci, in_dci) = (ep_out.dci(), ep_in.dci());
        let c = self.ctx;
        let input_phys = {
            let dev = self.devices.get(&slot).ok_or(UsbError::BadDescriptor)?;
            let input = &dev.input;
            input.zero();
            Self::write_ctx(input, c, 0, 1, 1 | 1 << out_dci | 1 << in_dci);
            Self::write_ctx(
                input,
                c,
                1,
                0,
                dev.route | (dev.speed as u32) << 20 | u32::from(out_dci.max(in_dci)) << 27,
            );
            Self::write_ctx(input, c, 1, 1, u32::from(dev.root_port) << 16);
            // Endpoint contexts: CErr 3, type 2 (bulk OUT) or 6 (bulk IN),
            // the largest packet; the ring with DCS = 1 (xHCI 1.2 6.2.3).
            for (ep, ring, ty) in [(ep_out, &out_ring, 2u32), (ep_in, &in_ring, 6u32)] {
                let at = usize::from(ep.dci()) + 1;
                let mp = u32::from(ep.max_packet);
                Self::write_ctx(input, c, at, 0, 0);
                Self::write_ctx(input, c, at, 1, 3 << 1 | ty << 3 | mp << 16);
                Self::write_ctx(input, c, at, 2, ring.phys() as u32 | 1);
                Self::write_ctx(input, c, at, 3, (ring.phys() >> 32) as u32);
                Self::write_ctx(input, c, at, 4, 3072);
            }
            input.phys()
        };
        self.command(Trb::command(kind::CONFIGURE_ENDPOINT, slot, input_phys))?;
        let name = format!("usb{}", NEXT_DISK.fetch_add(1, Ordering::Relaxed));
        if let Some(dev) = self.devices.get_mut(&slot) {
            dev.role = Role::Storage {
                out_dci,
                in_dci,
                out_ring,
                in_ring,
                wrapper,
                data,
                tag: 1,
                sector: 512,
                disk: Arc::new(super::storage::UsbDisk {
                    slot,
                    blocks: 0,
                    gone: core::sync::atomic::AtomicBool::new(false),
                }),
                name: name.clone(),
            };
        }
        use super::storage::scsi;
        let mut inquiry = [0u8; 36];
        self.bot(slot, &scsi::inquiry(36), true, &mut inquiry)?;
        let (vendor, product) = scsi::names(&inquiry);
        // A medium may need a moment.
        for _ in 0..10 {
            if self
                .bot(slot, &scsi::TEST_UNIT_READY, false, &mut [])
                .is_ok()
            {
                break;
            }
            spin(50_000_000);
        }
        let mut cap = [0u8; 8];
        self.bot(slot, &scsi::READ_CAPACITY_10, true, &mut cap)?;
        let (sectors, sector) = scsi::capacity(&cap).ok_or(UsbError::BadDescriptor)?;
        let blocks = sectors * u64::from(sector) / super::storage::BLOCK as u64;
        let disk = Arc::new(super::storage::UsbDisk {
            slot,
            blocks,
            gone: core::sync::atomic::AtomicBool::new(false),
        });
        if let Some(Role::Storage {
            sector: s, disk: d, ..
        }) = self.devices.get_mut(&slot).map(|d| &mut d.role)
        {
            *s = sector;
            *d = Arc::clone(&disk);
        }
        if let Some(h) = *STORAGE_HOOKS.lock() {
            (h.attach)(name.clone(), disk);
        }
        Ok(Some(format!(
            "{name}: {vendor} {product}, {} MiB in {sector}-byte sectors",
            (sectors * u64::from(sector)) >> 20
        )))
    }

    /// One bulk transfer of `len` bytes at `phys` on `slot`'s endpoint
    /// `dci`; the bytes moved.
    fn bulk(&mut self, slot: u8, dci: u8, phys: u64, len: u32) -> Result<u32, UsbError> {
        let at = {
            let dev = self.devices.get_mut(&slot).ok_or(UsbError::BadDescriptor)?;
            let Role::Storage {
                out_dci,
                out_ring,
                in_ring,
                ..
            } = &mut dev.role
            else {
                return Err(UsbError::BadDescriptor);
            };
            let ring = if dci == *out_dci { out_ring } else { in_ring };
            ring.push(Trb::normal(phys, len))
        };
        self.ring_doorbell(slot, dci);
        let e = self.wait_event(|e| {
            e.kind() == kind::TRANSFER_EVENT
                && e.slot() == slot
                && e.endpoint() == dci
                && e.parameter == at
        })?;
        match e.completion() {
            completion::SUCCESS | completion::SHORT_PACKET => {
                Ok(len.saturating_sub(e.status & 0xFF_FFFF))
            }
            c => Err(UsbError::Completion(c)),
        }
    }

    /// One SCSI command over Bulk-Only Transport: CBW, `data` (IN or OUT,
    /// at most 4 KiB), CSW; an error unless the status is good.
    fn bot(
        &mut self,
        slot: u8,
        cdb: &[u8],
        data_in: bool,
        data: &mut [u8],
    ) -> Result<(), UsbError> {
        use super::storage::{cbw, csw_status};
        let (out_dci, in_dci, wrapper, buffer, tag) = {
            let dev = self.devices.get_mut(&slot).ok_or(UsbError::BadDescriptor)?;
            let Role::Storage {
                out_dci,
                in_dci,
                wrapper,
                data,
                tag,
                ..
            } = &mut dev.role
            else {
                return Err(UsbError::BadDescriptor);
            };
            *tag = tag.wrapping_add(1);
            (*out_dci, *in_dci, wrapper.phys(), data.phys(), *tag)
        };
        let len = data.len().min(4096) as u32;
        let block = cbw(tag, len, data_in, cdb);
        self.storage_buffer(slot, true, |w| {
            let _ = w.write(0, &block);
        });
        self.bulk(slot, out_dci, wrapper, 31)?;
        if len > 0 {
            if !data_in {
                self.storage_buffer(slot, false, |d| {
                    let _ = d.write(0, &data[..len as usize]);
                });
            }
            self.bulk(slot, if data_in { in_dci } else { out_dci }, buffer, len)?;
            if data_in {
                self.storage_buffer(slot, false, |d| {
                    let _ = d.read(0, &mut data[..len as usize]);
                });
            }
        }
        self.bulk(slot, in_dci, wrapper + 512, 13)?;
        let mut csw = [0u8; 13];
        self.storage_buffer(slot, true, |w| {
            let _ = w.read(512, &mut csw);
        });
        match csw_status(&csw, tag) {
            Some(0) => Ok(()),
            Some(s) => Err(UsbError::Completion(0x80 | s)),
            None => Err(UsbError::BadDescriptor),
        }
    }

    /// Runs `f` on a storage device's wrapper page (`wrapper`) or data page.
    fn storage_buffer(&self, slot: u8, wrapper: bool, f: impl FnOnce(&DmaBuffer)) {
        if let Some(Role::Storage {
            wrapper: w, data, ..
        }) = self.devices.get(&slot).map(|d| &d.role)
        {
            f(if wrapper { w } else { data });
        }
    }

    /// Serves the disk requests that are waiting.
    fn serve_storage(&mut self) {
        use super::storage::Op;
        for job in super::storage::take_jobs() {
            let sector = match self.devices.get(&job.slot).map(|d| &d.role) {
                Some(Role::Storage { sector, .. }) => *sector,
                _ => {
                    *job.result.lock() = Some(false);
                    wake(job.waiter.load(Ordering::Acquire));
                    continue;
                }
            };
            let per = (super::storage::BLOCK as u32 / sector).max(1);
            let (write, block) = match job.op {
                Op::Read(b) => (false, b),
                Op::Write(b) => (true, b),
            };
            let lba = (block * u64::from(per)) as u32;
            let cdb = super::storage::scsi::rw10(write, lba, per as u16);
            let mut data = core::mem::take(&mut *job.data.lock());
            data.resize(super::storage::BLOCK, 0);
            let ok = self.bot(job.slot, &cdb, !write, &mut data).is_ok();
            *job.data.lock() = data;
            *job.result.lock() = Some(ok);
            wake(job.waiter.load(Ordering::Acquire));
        }
    }

    /// Queues a Normal TRB for the next report of `slot`'s HID endpoint.
    fn queue_report(&mut self, slot: u8) {
        let Some(dev) = self.devices.get_mut(&slot) else {
            return;
        };
        let Role::Hid {
            dci,
            ring,
            report,
            length,
            ..
        } = &mut dev.role
        else {
            return;
        };
        let dci = *dci;
        ring.push(Trb::normal(report.phys(), u32::from(*length)));
        self.ring_doorbell(slot, dci);
    }

    /// A transfer event that may be a HID report.
    fn hid_event(&mut self, e: Trb) {
        let slot = e.slot();
        let Some(dev) = self.devices.get_mut(&slot) else {
            return;
        };
        let Role::Hid {
            mouse,
            dci,
            report,
            length,
            keyboard,
            seen,
            ..
        } = &mut dev.role
        else {
            return;
        };
        if e.endpoint() != *dci {
            return;
        }
        match e.completion() {
            completion::SUCCESS | completion::SHORT_PACKET => {
                if !*seen {
                    *seen = true;
                    frame::kprintln!("usb: {}: first report", dev.path);
                }
                let n = usize::from(*length).saturating_sub(e.residual() as usize);
                let mut bytes = [0u8; 64];
                let _ = report.read(0, &mut bytes[..n.min(64)]);
                let bytes = &bytes[..n.min(64)];
                if *mouse {
                    if let Some(m) = hid::mouse_report(bytes) {
                        input::push(InputEvent::Mouse(m));
                    }
                } else {
                    for k in keyboard.report(bytes) {
                        input::push(InputEvent::Key(k));
                    }
                }
                self.queue_report(slot);
            }
            // A stalled or broken endpoint stops being polled.
            c => frame::kprintln!("usb: slot {slot}: report failed (completion {c})"),
        }
    }

    // ---- hubs ----

    fn setup_hub(&mut self, slot: u8) -> Result<(), UsbError> {
        let mut d = [0u8; 9];
        self.control(slot, Setup::get_hub_descriptor(9), &mut d)?;
        let ports = d[2].min(15);
        let power_good = u64::from(d[5]) * 2_000_000;
        let c = self.ctx;
        let input = {
            let dev = self.devices.get(&slot).ok_or(UsbError::BadDescriptor)?;
            let input = &dev.input;
            input.zero();
            Self::write_ctx(input, c, 0, 1, 1);
            // Hub bit 26, one context entry; number of ports.
            Self::write_ctx(
                input,
                c,
                1,
                0,
                dev.route | (dev.speed as u32) << 20 | 1 << 26 | 1 << 27,
            );
            Self::write_ctx(
                input,
                c,
                1,
                1,
                u32::from(dev.root_port) << 16 | u32::from(ports) << 24,
            );
            input.phys()
        };
        self.command(Trb::command(kind::CONFIGURE_ENDPOINT, slot, input))?;
        for p in 1..=ports {
            self.control(
                slot,
                Setup::port_feature(p, port_feature::POWER, true),
                &mut [],
            )?;
        }
        spin(power_good.max(20_000_000));
        if let Some(dev) = self.devices.get_mut(&slot) {
            dev.role = Role::Hub {
                ports,
                children: vec![None; usize::from(ports)],
            };
        }
        self.publish();
        self.poll_hub(slot);
        Ok(())
    }

    /// Checks every port of hub `slot` for devices that came or went.
    fn poll_hub(&mut self, slot: u8) {
        let Some(Role::Hub { ports, .. }) = self.devices.get(&slot).map(|d| &d.role) else {
            return;
        };
        for port in 1..=*ports {
            let mut st = [0u8; 4];
            if self
                .control(slot, Setup::get_port_status(port), &mut st)
                .is_err()
            {
                return;
            }
            let Some(status) = parse_port_status(&st) else {
                continue;
            };
            if status.connect_changed {
                let _ = self.control(
                    slot,
                    Setup::port_feature(port, port_feature::C_CONNECTION, false),
                    &mut [],
                );
            }
            let child = match self.devices.get(&slot).map(|d| &d.role) {
                Some(Role::Hub { children, .. }) => children[usize::from(port) - 1],
                _ => return,
            };
            match (status.connected, child) {
                (true, None) => self.attach_hub_port(slot, port),
                (false, Some(child)) => self.detach(child),
                (true, Some(child)) if status.connect_changed => {
                    self.detach(child);
                    self.attach_hub_port(slot, port);
                }
                _ => {}
            }
        }
    }

    fn attach_hub_port(&mut self, hub: u8, port: u8) {
        let reset = Setup::port_feature(port, port_feature::RESET, true);
        if self.control(hub, reset, &mut []).is_err() {
            return;
        }
        let mut status = None;
        for _ in 0..50 {
            spin(10_000_000);
            let mut st = [0u8; 4];
            if self
                .control(hub, Setup::get_port_status(port), &mut st)
                .is_err()
            {
                return;
            }
            let s = parse_port_status(&st);
            if s.is_some_and(|s| s.reset_changed || (!s.resetting && s.enabled)) {
                status = s;
                break;
            }
        }
        let _ = self.control(
            hub,
            Setup::port_feature(port, port_feature::C_RESET, false),
            &mut [],
        );
        let Some(status) = status.filter(|s| s.connected && s.enabled) else {
            return;
        };
        spin(10_000_000);
        let Some((path, root_port, route, depth)) = self.devices.get(&hub).map(|h| {
            (
                format!("{}.{port}", h.path),
                h.root_port,
                h.route | u32::from(port.min(15)) << (4 * u32::from(h.depth)),
                h.depth + 1,
            )
        }) else {
            return;
        };
        if depth > MAX_DEPTH {
            return;
        }
        if let Ok(child) = self.enumerate(
            path,
            status.speed,
            root_port,
            route,
            depth,
            Some((hub, port)),
        ) && let Some(Role::Hub { children, .. }) =
            self.devices.get_mut(&hub).map(|d| &mut d.role)
        {
            children[usize::from(port) - 1] = Some(child);
        }
    }

    // ---- root ports ----

    fn attach_root(&mut self, port: u8) {
        let sc = self.portsc(port);
        if sc & PORT_PED == 0 {
            // USB 2 ports need a reset to be enabled (USB 3 ones train
            // by themselves).
            self.set_portsc(port, (sc & PORT_KEEP) | PORT_PR);
            for _ in 0..50 {
                spin(10_000_000);
                if self.portsc(port) & PORT_PRC != 0 {
                    break;
                }
            }
        }
        let sc = self.portsc(port);
        self.set_portsc(port, (sc & PORT_KEEP) | (sc & PORT_CHANGES));
        if sc & (PORT_CCS | PORT_PED) != PORT_CCS | PORT_PED {
            return;
        }
        let Some(speed) = Speed::from_raw((sc >> 10) & 0xF) else {
            return;
        };
        spin(10_000_000);
        if let Ok(slot) = self.enumerate(format!("{port}"), speed, port, 0, 0, None) {
            self.root[usize::from(port)] = Some(slot);
        }
    }

    fn check_root_port(&mut self, port: u8) {
        let sc = self.portsc(port);
        self.set_portsc(port, (sc & PORT_KEEP) | (sc & PORT_CHANGES));
        let connected = sc & PORT_CCS != 0;
        match (connected, self.root[usize::from(port)]) {
            (true, None) => self.attach_root(port),
            (false, Some(slot)) => {
                self.detach(slot);
                self.root[usize::from(port)] = None;
            }
            (true, Some(slot)) if sc & PORT_CSC != 0 => {
                self.detach(slot);
                self.root[usize::from(port)] = None;
                self.attach_root(port);
            }
            _ => {}
        }
    }

    /// Frees a slot without telling anyone (enumeration failed).
    fn disable(&mut self, slot: u8) {
        let _ = self.command(Trb::command(kind::DISABLE_SLOT, slot, 0));
        let _ = self.dcbaa.write_u64(usize::from(slot) * 8, 0);
        self.devices.remove(&slot);
    }

    /// A device went away: its children too, then its slot.
    fn detach(&mut self, slot: u8) {
        let children: Vec<u8> = match self.devices.get(&slot).map(|d| &d.role) {
            Some(Role::Hub { children, .. }) => children.iter().flatten().copied().collect(),
            _ => Vec::new(),
        };
        for child in children {
            self.detach(child);
        }
        let Some(path) = self.devices.get(&slot).map(|d| d.path.clone()) else {
            return;
        };
        if let Some(Role::Storage { disk, name, .. }) = self.devices.get(&slot).map(|d| &d.role) {
            disk.gone.store(true, Ordering::Release);
            let name = name.clone();
            if let Some(h) = *STORAGE_HOOKS.lock() {
                (h.detach)(&name);
            }
        }
        if let Some((hub, port)) = self.devices.get(&slot).and_then(|d| d.parent)
            && let Some(Role::Hub { children, .. }) =
                self.devices.get_mut(&hub).map(|d| &mut d.role)
        {
            children[usize::from(port) - 1] = None;
        }
        self.disable(slot);
        self.publish();
        frame::kprintln!("usb: {path}: removed");
    }

    /// One round of the service loop.
    fn service(&mut self, last_hub_poll: &mut u64) {
        while let Some(e) = self.next_event() {
            self.dispatch(e);
        }
        let changed = core::mem::take(&mut self.changed_ports);
        for port in 1..=self.ports {
            if changed & (1 << port) != 0 {
                self.check_root_port(port);
            }
        }
        let now = crate::sync::hooks().map_or(0, |h| (h.now_ns)());
        if now.saturating_sub(*last_hub_poll) >= HUB_POLL_NS {
            *last_hub_poll = now;
            let hubs: Vec<u8> = self
                .devices
                .iter()
                .filter(|(_, d)| matches!(d.role, Role::Hub { .. }))
                .map(|(&s, _)| s)
                .collect();
            for hub in hubs {
                self.poll_hub(hub);
            }
        }
    }

    /// The controller's name (`xhci0`) and PCI address.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Finds the first xHCI controller (PCI class 0C/03/30), takes it from the
/// firmware, resets and starts it. Ports are scanned by [`run`].
pub fn probe() -> Option<Controller> {
    let f = *pci::devices().iter().find(|f| f.is(0x0C, 0x03, 0x30))?;
    match start(&f) {
        Ok(c) => Some(c),
        Err(why) => {
            frame::kprintln!("usb: {}: not started: {why}", f.address);
            None
        }
    }
}

fn start(f: &Function) -> Result<Controller, &'static str> {
    let regs = Arc::new(pci::map_bar(f, 0).map_err(|_| "BAR 0 cannot be mapped")?);
    pci::enable_bus_master(f);
    let cap = regs.read_u32(CAPLENGTH);
    let op = (cap & 0xFF) as usize;
    let params1 = regs.read_u32(HCSPARAMS1);
    let params2 = regs.read_u32(HCSPARAMS2);
    let cparams = regs.read_u32(HCCPARAMS1);
    let rt = (regs.read_u32(RTSOFF) & !0x1F) as usize;
    let db = (regs.read_u32(DBOFF) & !0x3) as usize;
    let max_slots = (params1 & 0xFF).min(32) as u8;
    let ports = (params1 >> 24) as u8;
    let ctx = if cparams & (1 << 2) != 0 { 64 } else { 32 };
    take_ownership(&regs, cparams);
    // Stop, then reset (5.4.1).
    let cmd = regs.read_u32(op + USBCMD);
    regs.write_u32(op + USBCMD, cmd & !CMD_RUN);
    if !crate::sync::spin_for(100_000_000, || regs.read_u32(op + USBSTS) & STS_HALTED != 0) {
        return Err("does not halt");
    }
    regs.write_u32(op + USBCMD, CMD_RESET);
    if !crate::sync::spin_for(1_000_000_000, || {
        regs.read_u32(op + USBCMD) & CMD_RESET == 0 && regs.read_u32(op + USBSTS) & STS_CNR == 0
    }) {
        return Err("reset does not finish");
    }
    regs.write_u32(op + CONFIG, u32::from(max_slots));
    let dcbaa = DmaBuffer::new(1).ok_or("no DMA memory")?;
    // Scratchpad buffers the controller asks for (HCSPARAMS2 bits 25:21
    // and 31:27).
    let scratch_count = ((params2 >> 21) & 0x1F) << 5 | (params2 >> 27);
    let mut scratch = Vec::new();
    if scratch_count > 0 {
        let array = DmaBuffer::new(1).ok_or("no DMA memory")?;
        for i in 0..scratch_count as usize {
            let page = DmaBuffer::new(1).ok_or("no DMA memory")?;
            let _ = array.write_u64(i * 8, page.phys());
            scratch.push(page);
        }
        let _ = dcbaa.write_u64(0, array.phys());
        scratch.push(array);
    }
    let commands = Ring::new().map_err(|_| "no DMA memory")?;
    let events = DmaBuffer::new(1).ok_or("no DMA memory")?;
    let erst = DmaBuffer::new(1).ok_or("no DMA memory")?;
    let _ = erst.write_u64(0, events.phys());
    let _ = erst.write_u32(8, RING_TRBS as u32);
    let c = Controller {
        regs: Arc::clone(&regs),
        op,
        rt,
        db,
        ports,
        ctx,
        dcbaa,
        _scratch: scratch,
        commands,
        events,
        _erst: erst,
        consumer: Consumer::new(RING_TRBS),
        devices: BTreeMap::new(),
        root: vec![None; usize::from(ports) + 1],
        changed_ports: 0,
        name: format!("xhci0 ({})", f.address),
    };
    c.op_write64(DCBAAP, c.dcbaa.phys());
    c.op_write64(CRCR, c.commands.phys() | 1);
    regs.write_u32(rt + ERSTSZ, 1);
    c.rt_write64(ERDP, c.events.phys());
    c.rt_write64(ERSTBA, c._erst.phys());
    // At most one interrupt per 250 microseconds (IMOD in 250 ns units).
    regs.write_u32(rt + IMOD, 1000);
    *IRQ.lock() = Some((Arc::clone(&regs), rt));
    let msi = pci::enable_msi(f, interrupt);
    regs.write_u32(rt + IMAN, IMAN_IP | IMAN_IE);
    regs.write_u32(op + USBSTS, STS_EINT);
    regs.write_u32(op + USBCMD, CMD_RUN | CMD_INTE);
    if !crate::sync::spin_for(100_000_000, || regs.read_u32(op + USBSTS) & STS_HALTED == 0) {
        return Err("does not run");
    }
    // Ports without power (PPC) get it.
    for port in 1..=ports {
        let sc = c.portsc(port);
        if sc & PORT_PP == 0 {
            c.set_portsc(port, (sc & PORT_KEEP) | PORT_PP);
        }
    }
    frame::kprintln!(
        "usb: {}: xHCI {}.{}, {ports} ports, {max_slots} slots, {}-byte contexts, {}",
        c.name,
        regs.read_u32(CAPLENGTH) >> 24,
        (regs.read_u32(CAPLENGTH) >> 16) & 0xFF,
        ctx,
        match msi {
            Ok(v) => format!("vector {v}"),
            Err(_) => String::from("polled"),
        }
    );
    Ok(c)
}

/// The USB Legacy Support capability (xHCI 1.2 7.1): asks the firmware to
/// let go (OS-owned semaphore, wait for BIOS-owned to clear) and turns its
/// SMIs off.
fn take_ownership(regs: &MmioRegion, cparams: u32) {
    let mut at = ((cparams >> 16) as usize) * 4;
    for _ in 0..64 {
        if at == 0 || at + 8 > regs.len() {
            return;
        }
        let v = regs.read_u32(at);
        if v & 0xFF == 1 {
            regs.write_u32(at, v | 1 << 24);
            crate::sync::spin_for(1_000_000_000, || regs.read_u32(at) & (1 << 16) == 0);
            let ctl = regs.read_u32(at + 4);
            regs.write_u32(at + 4, (ctl & !(1 | 1 << 4 | 7 << 13)) | 7 << 29);
            return;
        }
        let next = ((v >> 8) & 0xFF) as usize * 4;
        if next == 0 {
            return;
        }
        at += next;
    }
}

/// The controller's thread: scans the ports, then services events,
/// changes and hubs forever.
pub fn run(mut c: Controller) -> ! {
    spin(100_000_000);
    for port in 1..=c.ports {
        if c.portsc(port) & PORT_CCS != 0 {
            c.attach_root(port);
        }
    }
    let mut last_hub_poll = 0;
    loop {
        c.service(&mut last_hub_poll);
        c.serve_storage();
        wait_for(
            50_000_000,
            |me| WAITER.store(me, Ordering::Release),
            || c.peek_event().is_some() || !super::storage::JOBS.lock().is_empty(),
        );
    }
}
