// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! virtio-net over the modern virtio PCI interface (Virtual I/O Device
//! (VIRTIO) Specification 1.1: 4.1.4 PCI capabilities, 2.6 split
//! virtqueues, 5.1 network device).
//!
//! ```text
//!  PCI vendor capabilities (id 0x09): cfg_type 1 common, 2 notify, 3 ISR,
//!  4 device-specific; each names a BAR, an offset and a length.
//!  common config: feature selects 0x00-0x0C, msix_config 0x10,
//!    device_status 0x14, queue_select 0x16, queue_size 0x18,
//!    queue_msix_vector 0x1A, queue_enable 0x1C, queue_notify_off 0x1E,
//!    queue_desc 0x20, queue_driver 0x28, queue_device 0x30
//!  per queue, one DMA page: 32 descriptors (16 bytes) at 0, the
//!    available ring at 512, the used ring at 2048
//!  queue 0 receives, queue 1 transmits; every buffer starts with the
//!    12-byte virtio_net_hdr (all zero: no offloads)
//! ```

/// Descriptors per queue.
pub const QUEUE_SIZE: usize = 32;
/// Offset of the available ring in a queue's page.
pub const AVAIL: usize = QUEUE_SIZE * 16;
/// Offset of the used ring.
pub const USED: usize = 2048;
/// Bytes of `struct virtio_net_hdr` with VIRTIO_F_VERSION_1 (5.1.6).
pub const NET_HEADER: usize = 12;

/// Capability types (4.1.4).
pub mod cfg {
    /// Common configuration.
    pub const COMMON: u8 = 1;
    /// Notifications.
    pub const NOTIFY: u8 = 2;
    /// Interrupt status.
    pub const ISR: u8 = 3;
    /// Device-specific configuration.
    pub const DEVICE: u8 = 4;
}

/// One virtio PCI capability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cap {
    /// `cfg::*`.
    pub kind: u8,
    /// BAR index.
    pub bar: u8,
    /// Offset in the BAR.
    pub offset: u32,
    /// Length of the structure.
    pub length: u32,
    /// Notify capabilities only: `notify_off_multiplier`.
    pub multiplier: u32,
}

/// Decodes a vendor capability from its configuration-space words
/// (`words[0]` is at the capability's offset): `cap_vndr, cap_next,
/// cap_len, cfg_type`, `bar`, padding, `offset`, `length`, and for notify
/// capabilities `notify_off_multiplier`.
pub fn parse_cap(words: &[u32; 5]) -> Cap {
    let w0 = words[0].to_le_bytes();
    Cap {
        kind: w0[3],
        bar: words[1] as u8,
        offset: words[2],
        length: words[3],
        multiplier: if w0[3] == cfg::NOTIFY { words[4] } else { 0 },
    }
}

/// Device status bits (2.1).
pub mod status {
    /// The guest noticed the device.
    pub const ACKNOWLEDGE: u8 = 1;
    /// The guest has a driver.
    pub const DRIVER: u8 = 2;
    /// The driver is ready.
    pub const DRIVER_OK: u8 = 4;
    /// Feature negotiation is complete.
    pub const FEATURES_OK: u8 = 8;
    /// Something went wrong in the device.
    pub const NEEDS_RESET: u8 = 64;
    /// The driver gave up.
    pub const FAILED: u8 = 128;
}

/// Feature bits: the device has a MAC address (5.1.3).
pub const F_MAC: u64 = 1 << 5;
/// The link status field is valid.
pub const F_STATUS: u64 = 1 << 16;
/// The modern interface (6).
pub const F_VERSION_1: u64 = 1 << 32;

/// The features the driver accepts of those offered.
pub fn negotiate(offered: u64) -> Option<u64> {
    (offered & F_VERSION_1 != 0).then_some(offered & (F_VERSION_1 | F_MAC | F_STATUS))
}

/// Descriptor flag: the device writes the buffer (2.6.5).
pub const DESC_WRITE: u16 = 2;

/// A split-queue descriptor.
pub fn descriptor(addr: u64, len: u32, flags: u16) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[0..8].copy_from_slice(&addr.to_le_bytes());
    d[8..12].copy_from_slice(&len.to_le_bytes());
    d[12..14].copy_from_slice(&flags.to_le_bytes());
    d
}

#[cfg(target_os = "none")]
pub(super) use system::init;

#[cfg(target_os = "none")]
mod system {
    use super::*;
    use crate::net::{BUFFER_SIZE, NetDevice, NetError, wake_waiter};
    use crate::pci::system as pci;
    use crate::pci::{Bar, Function};
    use crate::sync::spin_for;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, Ordering};
    use frame::mm::dma::DmaBuffer;
    use frame::mm::mmio::{self, MmioRegion};
    use frame::sync::SpinLock;
    use frame::trap::TrapFrame;

    const DEVICE_FEATURE_SELECT: usize = 0x00;
    const DEVICE_FEATURE: usize = 0x04;
    const DRIVER_FEATURE_SELECT: usize = 0x08;
    const DRIVER_FEATURE: usize = 0x0C;
    const MSIX_CONFIG: usize = 0x10;
    const DEVICE_STATUS: usize = 0x14;
    const QUEUE_SELECT: usize = 0x16;
    const Q_SIZE: usize = 0x18;
    const QUEUE_MSIX_VECTOR: usize = 0x1A;
    const QUEUE_ENABLE: usize = 0x1C;
    const QUEUE_NOTIFY_OFF: usize = 0x1E;
    const QUEUE_DESC: usize = 0x20;
    const QUEUE_DRIVER: usize = 0x28;
    const QUEUE_DEVICE: usize = 0x30;
    const NO_VECTOR: u16 = 0xFFFF;

    struct Queue {
        page: DmaBuffer,
        buffers: DmaBuffer,
        /// Byte offset of this queue's notification register.
        notify: usize,
        /// Next free entry of the available ring (free-running).
        avail: u16,
        /// Used ring entries handled so far (free-running).
        used: u16,
    }

    impl Queue {
        fn new(notify: usize) -> Option<Queue> {
            Some(Queue {
                page: DmaBuffer::new(1)?,
                buffers: DmaBuffer::new(QUEUE_SIZE * BUFFER_SIZE / 4096)?,
                notify,
                avail: 0,
                used: 0,
            })
        }

        fn u16_at(&self, offset: usize) -> u16 {
            let mut b = [0u8; 2];
            let _ = self.page.read(offset, &mut b);
            u16::from_le_bytes(b)
        }

        fn set_u16(&self, offset: usize, v: u16) {
            let _ = self.page.write(offset, &v.to_le_bytes());
        }

        /// Puts descriptor `id` in the available ring.
        fn offer(&mut self, id: u16) {
            let slot = usize::from(self.avail) % QUEUE_SIZE;
            self.set_u16(AVAIL + 4 + 2 * slot, id);
            self.avail = self.avail.wrapping_add(1);
            self.set_u16(AVAIL + 2, self.avail);
        }

        /// The device's used index.
        fn used_index(&self) -> u16 {
            self.u16_at(USED + 2)
        }
    }

    struct State {
        rx: Queue,
        tx: Queue,
    }

    pub(in crate::net) struct VirtioNet {
        common: MmioRegion,
        notify: MmioRegion,
        isr: Option<MmioRegion>,
        device: Option<MmioRegion>,
        features: u64,
        mac: [u8; 6],
        msi: AtomicBool,
        state: SpinLock<State>,
    }

    fn interrupt(_: &TrapFrame) {
        wake_waiter();
    }

    impl VirtioNet {
        fn read16(&self, reg: usize) -> u16 {
            self.common.read_u16(reg)
        }

        fn write16(&self, reg: usize, v: u16) {
            self.common.write_u16(reg, v)
        }

        fn status(&self) -> u8 {
            self.common.read_u8(DEVICE_STATUS)
        }

        fn set_status(&self, s: u8) {
            self.common.write_u8(DEVICE_STATUS, s)
        }

        fn kick(&self, q: &Queue, index: u16) {
            self.notify.write_u16(q.notify, index);
        }

        /// Programs queue `index` (4.1.5.1.3).
        fn setup_queue(&self, index: u16, q: &Queue, vector: u16) -> Result<(), &'static str> {
            self.write16(QUEUE_SELECT, index);
            let max = self.read16(Q_SIZE);
            if usize::from(max) < QUEUE_SIZE {
                return Err("queue too small");
            }
            self.write16(Q_SIZE, QUEUE_SIZE as u16);
            let base = q.page.phys();
            for (reg, addr) in [
                (QUEUE_DESC, base),
                (QUEUE_DRIVER, base + AVAIL as u64),
                (QUEUE_DEVICE, base + USED as u64),
            ] {
                self.common.write_u32(reg, addr as u32);
                self.common.write_u32(reg + 4, (addr >> 32) as u32);
            }
            self.write16(QUEUE_MSIX_VECTOR, vector);
            self.write16(QUEUE_ENABLE, 1);
            Ok(())
        }
    }

    /// The capabilities of `f` by type.
    fn caps(f: &Function) -> Vec<Cap> {
        let cfg = pci::config();
        crate::pci::capabilities(cfg, f.address)
            .into_iter()
            .filter(|&(id, _)| id == 0x09)
            .map(|(_, at)| {
                let mut words = [0u32; 5];
                for (i, w) in words.iter_mut().enumerate() {
                    *w = cfg.read32(f.address, at + 4 * i as u16);
                }
                parse_cap(&words)
            })
            .collect()
    }

    fn map(f: &Function, cap: &Cap) -> Result<MmioRegion, &'static str> {
        let Some(Bar::Memory { addr, .. }) = crate::pci::bar(pci::config(), f, cap.bar) else {
            return Err("capability in a non-memory BAR");
        };
        crate::pci::enable(pci::config(), f.address, crate::pci::command::MEMORY_SPACE);
        mmio::map(addr + u64::from(cap.offset), cap.length.max(1) as usize)
            .map_err(|_| "cannot map a capability")
    }

    /// Resets and starts the device (3.1.1).
    pub(in crate::net) fn init(f: &Function) -> Result<Arc<VirtioNet>, &'static str> {
        let caps = caps(f);
        let find = |kind| caps.iter().find(|c| c.kind == kind).copied();
        let common_cap = find(cfg::COMMON).ok_or("no modern virtio interface")?;
        let notify_cap = find(cfg::NOTIFY).ok_or("no notify capability")?;
        pci::enable_bus_master(f);
        let common = map(f, &common_cap)?;
        let notify = map(f, &notify_cap)?;
        let isr = find(cfg::ISR).and_then(|c| map(f, &c).ok());
        let device = find(cfg::DEVICE).and_then(|c| map(f, &c).ok());
        common.write_u8(DEVICE_STATUS, 0);
        if !spin_for(100_000_000, || common.read_u8(DEVICE_STATUS) == 0) {
            return Err("reset failed");
        }
        common.write_u8(DEVICE_STATUS, status::ACKNOWLEDGE | status::DRIVER);
        common.write_u32(DEVICE_FEATURE_SELECT, 0);
        let low = common.read_u32(DEVICE_FEATURE);
        common.write_u32(DEVICE_FEATURE_SELECT, 1);
        let high = common.read_u32(DEVICE_FEATURE);
        let features =
            negotiate(u64::from(high) << 32 | u64::from(low)).ok_or("no VIRTIO_F_VERSION_1")?;
        common.write_u32(DRIVER_FEATURE_SELECT, 0);
        common.write_u32(DRIVER_FEATURE, features as u32);
        common.write_u32(DRIVER_FEATURE_SELECT, 1);
        common.write_u32(DRIVER_FEATURE, (features >> 32) as u32);
        let s = status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK;
        common.write_u8(DEVICE_STATUS, s);
        if common.read_u8(DEVICE_STATUS) & status::FEATURES_OK == 0 {
            common.write_u8(DEVICE_STATUS, status::FAILED);
            return Err("features refused");
        }
        let mut mac = [0u8; 6];
        if features & F_MAC != 0
            && let Some(dev) = &device
        {
            for (i, b) in mac.iter_mut().enumerate() {
                *b = dev.read_u8(i);
            }
        }
        let multiplier = notify_cap.multiplier as usize;
        common.write_u16(QUEUE_SELECT, 0);
        let rx_notify = usize::from(common.read_u16(QUEUE_NOTIFY_OFF)) * multiplier;
        common.write_u16(QUEUE_SELECT, 1);
        let tx_notify = usize::from(common.read_u16(QUEUE_NOTIFY_OFF)) * multiplier;
        let rx = Queue::new(rx_notify).ok_or("no DMA memory")?;
        let tx = Queue::new(tx_notify).ok_or("no DMA memory")?;
        let card = Arc::new(VirtioNet {
            common,
            notify,
            isr,
            device,
            features,
            mac,
            msi: AtomicBool::new(false),
            state: SpinLock::new(State { rx, tx }),
        });
        // MSI-X entry 0 for both queues (enable_msi programs entry 0).
        let msi = pci::enable_msi(f, interrupt).is_ok();
        let vector = if msi { 0 } else { NO_VECTOR };
        card.write16(MSIX_CONFIG, NO_VECTOR);
        {
            let mut st = card.state.lock();
            card.setup_queue(0, &st.rx, vector)?;
            card.setup_queue(1, &st.tx, vector)?;
            // Every receive buffer to the device.
            let rx = &mut st.rx;
            for i in 0..QUEUE_SIZE {
                let addr = rx.buffers.phys() + (i * BUFFER_SIZE) as u64;
                let _ = rx
                    .page
                    .write(i * 16, &descriptor(addr, BUFFER_SIZE as u32, DESC_WRITE));
                rx.offer(i as u16);
            }
        }
        card.set_status(s | status::DRIVER_OK);
        if card.status() & (status::NEEDS_RESET | status::FAILED) != 0 {
            return Err("device failed");
        }
        {
            let st = card.state.lock();
            card.kick(&st.rx, 0);
        }
        card.msi.store(
            msi && card.read16(QUEUE_MSIX_VECTOR) != NO_VECTOR,
            Ordering::Release,
        );
        Ok(card)
    }

    impl NetDevice for VirtioNet {
        fn name(&self) -> &'static str {
            "virtio-net"
        }

        fn mac(&self) -> [u8; 6] {
            self.mac
        }

        fn link_up(&self) -> bool {
            // Bit 0 of the status field (5.1.4) when the device offers
            // it; otherwise always up.
            match &self.device {
                Some(dev) if self.features & F_STATUS != 0 => dev.read_u16(6) & 1 != 0,
                _ => true,
            }
        }

        fn interrupts(&self) -> bool {
            self.msi.load(Ordering::Acquire)
        }

        fn send(&self, frame: &[u8]) -> Result<(), NetError> {
            if frame.is_empty() || frame.len() + NET_HEADER > BUFFER_SIZE {
                return Err(NetError::BadLength);
            }
            let mut st = self.state.lock();
            let tx = &mut st.tx;
            let room = || usize::from(tx.avail.wrapping_sub(tx.used_index())) < QUEUE_SIZE;
            if !room() && !spin_for(100_000_000, room) {
                return Err(NetError::Busy);
            }
            tx.used = tx.used_index();
            let id = usize::from(tx.avail) % QUEUE_SIZE;
            let offset = id * BUFFER_SIZE;
            let _ = tx.buffers.write(offset, &[0u8; NET_HEADER]);
            let _ = tx.buffers.write(offset + NET_HEADER, frame);
            let addr = tx.buffers.phys() + offset as u64;
            let _ = tx.page.write(
                id * 16,
                &descriptor(addr, (NET_HEADER + frame.len()) as u32, 0),
            );
            tx.offer(id as u16);
            let notify = tx.notify;
            self.notify.write_u16(notify, 1);
            Ok(())
        }

        fn receive(&self, buf: &mut [u8]) -> Option<usize> {
            // Reading the ISR acknowledges an INTx interrupt (4.1.4.5).
            if let Some(isr) = &self.isr
                && !self.interrupts()
            {
                let _ = isr.read_u8(0);
            }
            let mut st = self.state.lock();
            let rx = &mut st.rx;
            loop {
                if rx.used == rx.used_index() {
                    return None;
                }
                let slot = usize::from(rx.used) % QUEUE_SIZE;
                let mut entry = [0u8; 8];
                let _ = rx.page.read(USED + 4 + 8 * slot, &mut entry);
                let id = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
                let len = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as usize;
                rx.used = rx.used.wrapping_add(1);
                let got = if id < QUEUE_SIZE && len > NET_HEADER && len - NET_HEADER <= buf.len() {
                    let n = len - NET_HEADER;
                    let _ = rx
                        .buffers
                        .read(id * BUFFER_SIZE + NET_HEADER, &mut buf[..n]);
                    Some(n)
                } else {
                    None
                };
                if id < QUEUE_SIZE {
                    rx.offer(id as u16);
                    let notify = rx.notify;
                    self.notify.write_u16(notify, 0);
                }
                if got.is_some() {
                    return got;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_words() {
        // vndr 0x09, next 0x50, len 20, type notify; BAR 4; offset 0x3000,
        // length 0x1000, multiplier 4.
        let cap = parse_cap(&[0x0214_5009, 4, 0x3000, 0x1000, 4]);
        assert_eq!(
            cap,
            Cap {
                kind: cfg::NOTIFY,
                bar: 4,
                offset: 0x3000,
                length: 0x1000,
                multiplier: 4
            }
        );
        let common = parse_cap(&[0x0110_6009, 4, 0, 0x1000, 0xFFFF]);
        assert_eq!((common.kind, common.multiplier), (cfg::COMMON, 0));
    }

    #[test]
    fn feature_negotiation() {
        assert_eq!(negotiate(F_MAC), None, "legacy-only devices are refused");
        let offered = F_VERSION_1 | F_MAC | F_STATUS | 1 << 0 | 1 << 15;
        assert_eq!(negotiate(offered), Some(F_VERSION_1 | F_MAC | F_STATUS));
    }

    #[test]
    fn ring_layout_fits_a_page() {
        const { assert!(AVAIL + 6 + 2 * QUEUE_SIZE <= USED) };
        const { assert!(USED + 6 + 8 * QUEUE_SIZE <= 4096) };
        let d = descriptor(0xABCD_0000, 2048, DESC_WRITE);
        assert_eq!(&d[8..12], &2048u32.to_le_bytes());
        assert_eq!(&d[12..14], &[2, 0]);
    }
}
