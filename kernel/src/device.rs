// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! User-space drivers (P13 K11, `docs/devices.md`): a `Device` object is
//! one PCI function lent to a program, and a `NetTap` makes a user-space
//! network card the kernel's card.
//!
//! ```text
//!  DeviceOpen("00:03.0", DEVICES) ─► Device (one owner; not if a kernel
//!                                     driver mapped its BARs)
//!  DeviceConfig     configuration space (1, 2, 4 bytes)
//!  DeviceMapBar     a memory BAR into the program (uncached)
//!  DeviceDma        contiguous memory + its physical address
//!  DeviceIrq(port)  MSI → kernel vector → (devirq thread) → DEVICE_IRQ
//!  DeviceReset      bus master and MSI off (also when the handle goes)
//!
//!  NetAttach(port, mac) ─► NetTap; the kernel's net uses it as its card:
//!     kernel sends: frame queued ─► NET_TX_READY to port ─► NetTake
//!     card receives:            NetDeliver ─► kernel's stack
//! ```
//!
//! There is no IOMMU: a device that masters the bus reaches all of
//! memory, so a program holding a `Device` is trusted with the machine.
//! The `DEVICES` capability (init gives it only to the drivers it starts)
//! is the gate. The interrupt handler only counts and wakes a kernel
//! thread; it never touches the device.

use crate::object::{Entry, KernelObject};
use crate::task::{self, Process};
use abi::{Error, Handle, Message, Rights, tag};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use drivers::pci::{Address, Function};
use frame::mm::shared::SharedMemory;
use frame::sync::SpinLock;

/// A PCI function lent to a program.
pub struct Device {
    function: Function,
    /// The MSI vector and the port its interrupts go to.
    irq: SpinLock<Option<(u8, Arc<crate::object::port::Port>)>>,
}

/// Functions lent out now.
static OPEN: SpinLock<Vec<Address>> = SpinLock::new(Vec::new());
/// Per vector (index = vector): interrupts not reported yet.
static PENDING: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
/// Vectors owned by devices: (vector, device).
static VECTORS: SpinLock<Vec<(u8, Arc<Device>)>> = SpinLock::new(Vec::new());
/// The thread that turns interrupts into messages.
static DEVIRQ: AtomicU64 = AtomicU64::new(0);
static DEVIRQ_STARTED: AtomicBool = AtomicBool::new(false);

impl Drop for Device {
    fn drop(&mut self) {
        reset(&self.function);
        if let Some((vector, _)) = self.irq.lock().take() {
            frame::irq::unregister(vector);
        }
        OPEN.lock().retain(|a| *a != self.function.address);
    }
}

/// Bus mastering and MSI off.
fn reset(f: &Function) {
    let cfg = drivers::pci::system::config();
    let cmd = drivers::pci::read16(cfg, f.address, 0x04);
    drivers::pci::write16(
        cfg,
        f.address,
        0x04,
        cmd & !drivers::pci::command::BUS_MASTER,
    );
    if let Some(cap) = drivers::pci::find_capability(cfg, f.address, drivers::pci::capability::MSI)
    {
        let control = drivers::pci::read16(cfg, f.address, cap + 2);
        drivers::pci::write16(cfg, f.address, cap + 2, control & !1);
    }
    if let Some(cap) =
        drivers::pci::find_capability(cfg, f.address, drivers::pci::capability::MSI_X)
    {
        let control = drivers::pci::read16(cfg, f.address, cap + 2);
        drivers::pci::write16(cfg, f.address, cap + 2, control & !(1 << 15));
    }
}

fn device_of(process: &Process, handle: u64) -> Result<Arc<Device>, Error> {
    let handles = process.handles().lock();
    match &handles.get(Handle::from_raw(handle))?.object {
        KernelObject::Device(d) => Ok(Arc::clone(d)),
        _ => Err(Error::WrongType),
    }
}

fn insert(process: &Process, object: KernelObject) -> Result<u64, Error> {
    let entry = Entry {
        object,
        rights: Rights::READ | Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER,
    };
    Ok(process.handles().lock().insert(entry)?.raw())
}

/// `DeviceOpen(path, len, devices)`.
pub fn device_open(process: &Process, ptr: u64, len: u64, cap: u64) -> Result<u64, Error> {
    crate::object::check_capability(process, cap, abi::capability::DEVICES)?;
    if len == 0 || len > 32 {
        return Err(Error::InvalidArgument);
    }
    let mut buf = alloc::vec![0u8; len as usize];
    process
        .space()
        .read(ptr, &mut buf)
        .map_err(|_| Error::BadAddress)?;
    let text = core::str::from_utf8(&buf).map_err(|_| Error::InvalidArgument)?;
    let (bus, dev, func) = drivers::pci::parse_location(text).ok_or(Error::InvalidArgument)?;
    let function = *drivers::pci::system::devices()
        .iter()
        .find(|f| f.address.bus == bus && f.address.device == dev && f.address.function == func)
        .ok_or(Error::NotFound)?;
    // Bridges keep the buses together; the display is the window server's.
    if function.class == 0x06 || function.class == 0x03 {
        return Err(Error::AccessDenied);
    }
    if drivers::pci::system::is_claimed(&function) {
        return Err(Error::Busy);
    }
    {
        let mut open = OPEN.lock();
        if open.contains(&function.address) {
            return Err(Error::Busy);
        }
        open.push(function.address);
    }
    let device = Arc::new(Device {
        function,
        irq: SpinLock::new(None),
    });
    insert(process, KernelObject::Device(device)).inspect_err(|_| {
        OPEN.lock().retain(|a| *a != function.address);
    })
}

/// `DeviceConfig(device, offset, size | WRITE, value)`.
pub fn device_config(
    process: &Process,
    handle: u64,
    offset: u64,
    size: u64,
    value: u64,
) -> Result<u64, Error> {
    let d = device_of(process, handle)?;
    let write = size & abi::device::WRITE != 0;
    let size = size & 0xFF;
    if !matches!(size, 1 | 2 | 4) || offset > 0xFFF || !offset.is_multiple_of(size) {
        return Err(Error::InvalidArgument);
    }
    let cfg = drivers::pci::system::config();
    let at = d.function.address;
    let offset = offset as u16;
    // The BARs stay where the firmware put them (moving one could make it
    // cover RAM).
    if write && (0x10..0x28).contains(&offset) {
        return Err(Error::AccessDenied);
    }
    let aligned = offset & !3;
    let shift = u32::from(offset & 3) * 8;
    let mask: u32 = match size {
        1 => 0xFF,
        2 => 0xFFFF,
        _ => u32::MAX,
    };
    let word = cfg.read32(at, aligned);
    if write {
        let new = (word & !(mask << shift)) | ((value as u32 & mask) << shift);
        cfg.write32(at, aligned, new);
        Ok(0)
    } else {
        Ok(u64::from((word >> shift) & mask))
    }
}

/// `DeviceMapBar(device, bar)`.
pub fn device_map_bar(
    process: &Process,
    handle: u64,
    bar: u64,
    size_ptr: u64,
) -> Result<u64, Error> {
    let d = device_of(process, handle)?;
    if bar > 5 {
        return Err(Error::InvalidArgument);
    }
    let cfg = drivers::pci::system::config();
    let Some(drivers::pci::Bar::Memory { addr, size, .. }) =
        drivers::pci::bar(cfg, &d.function, bar as u8)
    else {
        return Err(Error::NotSupported);
    };
    let range = frame::mm::device::bar_range(addr, size).ok_or(Error::AccessDenied)?;
    process
        .space()
        .write(size_ptr, &size.to_le_bytes())
        .map_err(|_| Error::BadAddress)?;
    let virt = process
        .maps()
        .lock()
        .alloc(range.size())
        .ok_or(Error::OutOfMemory)?;
    if process
        .space()
        .map_device(virt, &range, true, false)
        .is_err()
    {
        process.maps().lock().free(virt);
        return Err(Error::OutOfMemory);
    }
    drivers::pci::enable(cfg, d.function.address, drivers::pci::command::MEMORY_SPACE);
    Ok(virt + (addr & 0xFFF))
}

/// `DeviceDma(device, pages, phys_ptr)`.
pub fn device_dma(process: &Process, handle: u64, pages: u64, phys_ptr: u64) -> Result<u64, Error> {
    let d = device_of(process, handle)?;
    if pages == 0 || pages > 1024 {
        return Err(Error::InvalidArgument);
    }
    let memory = SharedMemory::new_contiguous(pages as usize).ok_or(Error::OutOfMemory)?;
    process
        .space()
        .write(phys_ptr, &memory.phys().to_le_bytes())
        .map_err(|_| Error::BadAddress)?;
    drivers::pci::system::enable_bus_master(&d.function);
    let entry = Entry {
        object: KernelObject::Memory(memory),
        rights: Rights::MAP | Rights::READ | Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER,
    };
    Ok(process.handles().lock().insert(entry)?.raw())
}

/// The handler of every device vector: count, wake the devirq thread.
fn interrupt(frame: &frame::trap::TrapFrame) {
    let v = (frame.vector & 0xFF) as usize;
    PENDING[v].fetch_add(1, Ordering::AcqRel);
    task::wake(DEVIRQ.load(Ordering::Acquire));
}

/// The devirq thread: interrupts become `DEVICE_IRQ` messages.
fn devirq() {
    DEVIRQ.store(task::current_id(), Ordering::Release);
    loop {
        let vectors: Vec<(u8, Arc<Device>)> = VECTORS.lock().clone();
        for (v, d) in vectors {
            let n = PENDING[usize::from(v)].swap(0, Ordering::AcqRel);
            if n == 0 {
                continue;
            }
            let port = d.irq.lock().as_ref().map(|(_, p)| Arc::clone(p));
            if let Some(port) = port {
                let mut data = [0u8; 16];
                data[..8].copy_from_slice(&u64::from(v).to_le_bytes());
                data[8..].copy_from_slice(&n.to_le_bytes());
                let msg = Message::with_data(tag::DEVICE_IRQ, &data);
                let _ = port.send(crate::object::port::QueuedMessage::plain(msg));
            }
        }
        // A missed wake-up costs at most this long.
        task::block_until(Some(frame::time::now_ns() + 50_000_000));
    }
}

/// `DeviceIrq(device, port)`.
pub fn device_irq(process: &Process, handle: u64, port: u64) -> Result<u64, Error> {
    let d = device_of(process, handle)?;
    let port = {
        let handles = process.handles().lock();
        let entry = handles.get(Handle::from_raw(port))?;
        if !entry.rights.contains(Rights::SEND) {
            return Err(Error::AccessDenied);
        }
        entry.object.as_port().cloned().ok_or(Error::WrongType)?
    };
    if d.irq.lock().is_some() {
        return Err(Error::Busy);
    }
    if !DEVIRQ_STARTED.swap(true, Ordering::AcqRel)
        && let Err(e) = task::spawn("devirq", devirq)
    {
        DEVIRQ_STARTED.store(false, Ordering::Release);
        frame::kprintln!("device: cannot start the interrupt thread: {e:?}");
        return Err(Error::OutOfMemory);
    }
    let vector = drivers::pci::system::enable_msi(&d.function, interrupt)
        .map_err(|_| Error::NotSupported)?;
    *d.irq.lock() = Some((vector, port));
    VECTORS.lock().push((vector, Arc::clone(&d)));
    Ok(u64::from(vector))
}

/// `DeviceReset(device)`.
pub fn device_reset(process: &Process, handle: u64) -> Result<u64, Error> {
    let d = device_of(process, handle)?;
    reset(&d.function);
    if let Some((vector, _)) = d.irq.lock().take() {
        frame::irq::unregister(vector);
        VECTORS.lock().retain(|(v, _)| *v != vector);
    }
    Ok(0)
}

/// A network card in user space, as the kernel's net sees it.
pub struct NetTap {
    mac: [u8; 6],
    port: Arc<crate::object::port::Port>,
    rx: SpinLock<VecDeque<Vec<u8>>>,
    tx: SpinLock<VecDeque<Vec<u8>>>,
    gone: AtomicBool,
}

/// Frames waiting in either direction, at most.
const TAP_QUEUE: usize = 256;

impl drivers::net::NetDevice for NetTap {
    fn name(&self) -> &'static str {
        "user"
    }
    fn mac(&self) -> [u8; 6] {
        self.mac
    }
    fn link_up(&self) -> bool {
        !self.gone.load(Ordering::Relaxed)
    }
    fn send(&self, frame: &[u8]) -> Result<(), drivers::net::NetError> {
        let was_empty = {
            let mut tx = self.tx.lock();
            if tx.len() >= TAP_QUEUE {
                return Err(drivers::net::NetError::Busy);
            }
            let empty = tx.is_empty();
            tx.push_back(frame.to_vec());
            empty
        };
        if was_empty {
            let msg = Message::with_data(tag::NET_TX_READY, &[]);
            let _ = self
                .port
                .send(crate::object::port::QueuedMessage::plain(msg));
        }
        Ok(())
    }
    fn receive(&self, buf: &mut [u8]) -> Option<usize> {
        let f = self.rx.lock().pop_front()?;
        let n = f.len().min(buf.len());
        buf[..n].copy_from_slice(&f[..n]);
        Some(n)
    }
    fn interrupts(&self) -> bool {
        true
    }
}

/// The tap's handle went away: the card goes too.
pub struct TapHandle(Arc<NetTap>);

impl Drop for TapHandle {
    fn drop(&mut self) {
        self.0.gone.store(true, Ordering::Release);
        crate::net::usb_detached(Arc::clone(&self.0) as Arc<dyn drivers::net::NetDevice>);
    }
}

fn tap_of(process: &Process, handle: u64) -> Result<Arc<TapHandle>, Error> {
    let handles = process.handles().lock();
    match &handles.get(Handle::from_raw(handle))?.object {
        KernelObject::NetTap(t) => Ok(Arc::clone(t)),
        _ => Err(Error::WrongType),
    }
}

/// `NetAttach(port, mac_ptr, devices)`.
pub fn net_attach(process: &Process, port: u64, mac_ptr: u64, cap: u64) -> Result<u64, Error> {
    crate::object::check_capability(process, cap, abi::capability::DEVICES)?;
    let port = {
        let handles = process.handles().lock();
        let entry = handles.get(Handle::from_raw(port))?;
        if !entry.rights.contains(Rights::SEND) {
            return Err(Error::AccessDenied);
        }
        entry.object.as_port().cloned().ok_or(Error::WrongType)?
    };
    let mut mac = [0u8; 6];
    process
        .space()
        .read(mac_ptr, &mut mac)
        .map_err(|_| Error::BadAddress)?;
    let tap = Arc::new(NetTap {
        mac,
        port,
        rx: SpinLock::new(VecDeque::new()),
        tx: SpinLock::new(VecDeque::new()),
        gone: AtomicBool::new(false),
    });
    let h = insert(
        process,
        KernelObject::NetTap(Arc::new(TapHandle(Arc::clone(&tap)))),
    )?;
    // The network thread takes it like a USB adapter that came.
    crate::net::usb_attached(tap as Arc<dyn drivers::net::NetDevice>);
    Ok(h)
}

/// `NetDeliver(tap, buf, len)`.
pub fn net_deliver(process: &Process, handle: u64, buf: u64, len: u64) -> Result<u64, Error> {
    let tap = tap_of(process, handle)?;
    if len == 0 || len > 1518 {
        return Err(Error::InvalidArgument);
    }
    let mut frame = alloc::vec![0u8; len as usize];
    process
        .space()
        .read(buf, &mut frame)
        .map_err(|_| Error::BadAddress)?;
    {
        let mut rx = tap.0.rx.lock();
        if rx.len() >= TAP_QUEUE {
            return Err(Error::Full);
        }
        rx.push_back(frame);
    }
    drivers::net::wake_waiter();
    Ok(0)
}

/// `NetTake(tap, buf, len)`.
pub fn net_take(process: &Process, handle: u64, buf: u64, len: u64) -> Result<u64, Error> {
    let tap = tap_of(process, handle)?;
    let Some(frame) = tap.0.tx.lock().pop_front() else {
        return Ok(0);
    };
    if frame.len() as u64 > len {
        return Err(Error::InvalidArgument);
    }
    process
        .space()
        .write(buf, &frame)
        .map_err(|_| Error::BadAddress)?;
    Ok(frame.len() as u64)
}
