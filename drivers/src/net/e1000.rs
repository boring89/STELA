// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Intel 8254x/82574/I21x Ethernet controllers with legacy descriptors
//! (PCI/PCI-X Family of Gigabit Ethernet Controllers Software Developer's
//! Manual, "8254x SDM", sections 3.2-3.4 and 13; the 82574 and I219
//! datasheets keep the same registers for what is used here).
//!
//! ```text
//!  BAR0 registers: CTRL 0x0 STATUS 0x8 ICR 0xC0 IMS 0xD0 IMC 0xD8
//!                  RCTL 0x100 TCTL 0x400 TIPG 0x410
//!                  RDBAL/RDBAH/RDLEN/RDH/RDT 0x2800.. TDBAL.. 0x3800..
//!                  MTA 0x5200 (128 words) RAL0/RAH0 0x5400
//!  one DMA page: 32 receive descriptors (16 bytes each), then 8 transmit
//!  DMA buffers: 32 + 8 of 2048 bytes
//!
//!  receive descriptor: address(8) length(2) checksum(2) status(1) errors(1) special(2)
//!  transmit descriptor: address(8) length(2) cso(1) command(1) status(1) css(1) special(2)
//! ```
//!
//! The card owns the receive descriptors from RDH up to (not including)
//! RDT and sets DD (descriptor done) in a descriptor's status when it
//! filled it; the driver hands it back by moving RDT. Transmit is the
//! mirror image: the driver fills descriptors and moves TDT, the card sets
//! DD when sent.

/// Receive descriptors.
pub const RX_COUNT: usize = 32;
/// Transmit descriptors.
pub const TX_COUNT: usize = 8;

/// Descriptor status: descriptor done.
pub const STATUS_DD: u8 = 0x01;
/// Receive status: end of packet.
pub const STATUS_EOP: u8 = 0x02;
/// Transmit command: end of packet, insert FCS (CRC), report status
/// (8254x SDM 3.3.3.1).
pub const TX_CMD: u8 = 0x01 | 0x02 | 0x08;

/// A transmit descriptor for `len` bytes at `addr`.
pub fn tx_descriptor(addr: u64, len: u16) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[0..8].copy_from_slice(&addr.to_le_bytes());
    d[8..10].copy_from_slice(&len.to_le_bytes());
    d[11] = TX_CMD;
    d
}

/// A receive descriptor pointing at `addr`, empty.
pub fn rx_descriptor(addr: u64) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[0..8].copy_from_slice(&addr.to_le_bytes());
    d
}

/// A filled receive descriptor: `Some(Ok(length))` for a good whole
/// frame, `Some(Err(()))` for one to drop (errors, or split over
/// descriptors), `None` while the card has not filled it.
pub fn rx_done(d: &[u8; 16]) -> Option<Result<usize, ()>> {
    let status = d[12];
    if status & STATUS_DD == 0 {
        return None;
    }
    let len = usize::from(u16::from_le_bytes([d[8], d[9]]));
    if status & STATUS_EOP == 0 || d[13] != 0 || len > super::BUFFER_SIZE {
        return Some(Err(()));
    }
    Some(Ok(len))
}

/// The MAC address from Receive Address Low/High 0, if RAH's Address
/// Valid bit (31) is set (8254x SDM 13.5.3).
pub fn mac_from(ral: u32, rah: u32) -> Option<[u8; 6]> {
    if rah & (1 << 31) == 0 {
        return None;
    }
    let l = ral.to_le_bytes();
    let h = rah.to_le_bytes();
    Some([l[0], l[1], l[2], l[3], h[0], h[1]])
}

#[cfg(target_os = "none")]
pub(super) use system::init;

#[cfg(target_os = "none")]
mod system {
    use super::*;
    use crate::net::{BUFFER_SIZE, Family, NetDevice, NetError, wake_waiter};
    use crate::pci::Function;
    use crate::pci::system as pci;
    use crate::sync::spin_for;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};
    use frame::mm::dma::DmaBuffer;
    use frame::mm::mmio::MmioRegion;
    use frame::sync::{Once, SpinLock};
    use frame::trap::TrapFrame;

    const CTRL: usize = 0x0000;
    const STATUS: usize = 0x0008;
    const ICR: usize = 0x00C0;
    const IMS: usize = 0x00D0;
    const IMC: usize = 0x00D8;
    const RCTL: usize = 0x0100;
    const TCTL: usize = 0x0400;
    const TIPG: usize = 0x0410;
    const RDBAL: usize = 0x2800;
    const RDBAH: usize = 0x2804;
    const RDLEN: usize = 0x2808;
    const RDH: usize = 0x2810;
    const RDT: usize = 0x2818;
    const TDBAL: usize = 0x3800;
    const TDBAH: usize = 0x3804;
    const TDLEN: usize = 0x3808;
    const TDH: usize = 0x3810;
    const TDT: usize = 0x3818;
    const TXDCTL: usize = 0x3828;
    const TARC0: usize = 0x3840;
    const MTA: usize = 0x5200;
    const RAL0: usize = 0x5400;
    const RAH0: usize = 0x5404;
    /// I219 (PCH) registers used by the flush workaround (FreeBSD
    /// `em_flush_desc_rings`, Intel I218/I219 specification update).
    const FEXTNVM11: usize = 0x5BBC;
    const IOSFPC: usize = 0x0F28;
    const RXDCTL: usize = 0x2828;
    /// PCI configuration register of the I219's descriptor ring status.
    const PCI_DESC_RING_STATUS: u16 = 0xE4;

    const CTRL_ASDE: u32 = 1 << 5;
    const CTRL_SLU: u32 = 1 << 6;
    const CTRL_FRCSPD: u32 = 1 << 11;
    const CTRL_FRCDPX: u32 = 1 << 12;
    const CTRL_RST: u32 = 1 << 26;
    const STATUS_LU: u32 = 1 << 1;
    const RCTL_EN: u32 = 1 << 1;
    const RCTL_BAM: u32 = 1 << 15;
    const RCTL_SECRC: u32 = 1 << 26;
    const TCTL_EN: u32 = 1 << 1;
    const TCTL_PSP: u32 = 1 << 3;
    /// Collision threshold 15, collision distance 64 (full duplex), the
    /// 8254x SDM 13.4.33 recommendations.
    const TCTL_CT_COLD: u32 = 0x0F << 4 | 0x40 << 12;
    const TCTL_RTLC: u32 = 1 << 24;
    /// Interrupt causes: link status change, receive descriptor minimum
    /// threshold, receiver overrun, receive timer (8254x SDM 13.4.17).
    const INTERRUPTS: u32 = 1 << 2 | 1 << 4 | 1 << 6 | 1 << 7;

    struct Rings {
        rx_next: usize,
        tx_next: usize,
    }

    pub(in crate::net) struct E1000 {
        name: &'static str,
        regs: MmioRegion,
        descriptors: DmaBuffer,
        buffers: DmaBuffer,
        mac: [u8; 6],
        msi: AtomicBool,
        rings: SpinLock<Rings>,
    }

    const TX_RING: usize = RX_COUNT * 16;
    const TX_BUFFERS: usize = RX_COUNT * BUFFER_SIZE;

    /// The card, for the interrupt handler.
    static CARD: Once<Arc<E1000>> = Once::new();

    /// MSI: reading ICR acknowledges every cause (8254x SDM 13.4.17).
    fn interrupt(_: &TrapFrame) {
        if let Some(card) = CARD.get() {
            let _ = card.regs.read_u32(ICR);
        }
        wake_waiter();
    }

    impl E1000 {
        fn read(&self, reg: usize) -> u32 {
            self.regs.read_u32(reg)
        }

        fn write(&self, reg: usize, value: u32) {
            self.regs.write_u32(reg, value)
        }

        fn descriptor(&self, offset: usize) -> [u8; 16] {
            let mut d = [0u8; 16];
            let _ = self.descriptors.read(offset, &mut d);
            d
        }

        /// I219 only: a reset hangs a chip still waiting for descriptors
        /// the firmware's driver left; one dummy descriptor through our
        /// (already programmed) transmit ring frees it, then a moment of
        /// receive with FreeBSD's thresholds.
        fn flush_rings(&self, f: &Function) {
            self.write(FEXTNVM11, self.read(FEXTNVM11) | 1 << 13);
            let cfg = pci::config();
            let required = || crate::pci::read16(cfg, f.address, PCI_DESC_RING_STATUS) & 0x100 != 0;
            if !required() {
                return;
            }
            let ring = self.descriptors.phys() + TX_RING as u64;
            let mut d = tx_descriptor(ring, 512);
            d[11] = 0x02; // IFCS only
            let _ = self.descriptors.write(TX_RING, &d);
            self.write(TCTL, self.read(TCTL) | TCTL_EN);
            self.write(TDT, 1);
            spin_for(250_000, || false);
            if required() {
                let rctl = self.read(RCTL);
                self.write(RCTL, rctl & !RCTL_EN);
                spin_for(150_000, || false);
                self.write(
                    RXDCTL,
                    (self.read(RXDCTL) & 0xFFFF_C000) | 0x1F | 1 << 8 | 1 << 24,
                );
                self.write(RCTL, rctl | RCTL_EN);
                spin_for(150_000, || false);
                self.write(RCTL, rctl & !RCTL_EN);
            }
            let _ = self.descriptors.write(TX_RING, &[0u8; 16]);
            self.write(TCTL, 0);
            self.write(TDH, 0);
            self.write(TDT, 0);
        }
    }

    /// Resets and starts the card.
    pub(in crate::net) fn init(f: &Function, family: Family) -> Result<Arc<E1000>, &'static str> {
        let (name, pch) = match family {
            Family::E1000 => ("e1000", false),
            Family::E1000e { pch } => ("e1000e", pch),
            Family::Virtio => return Err("not an e1000"),
        };
        pci::enable_bus_master(f);
        let regs = pci::map_bar(f, 0).map_err(|_| "cannot map BAR0")?;
        let descriptors = DmaBuffer::new(1).ok_or("no DMA memory")?;
        let buffers =
            DmaBuffer::new((RX_COUNT + TX_COUNT) * BUFFER_SIZE / 4096).ok_or("no DMA memory")?;
        let mut card = E1000 {
            name,
            regs,
            descriptors,
            buffers,
            mac: [0; 6],
            msi: AtomicBool::new(false),
            rings: SpinLock::new(Rings {
                rx_next: 0,
                tx_next: 0,
            }),
        };
        let desc_phys = card.descriptors.phys();
        let buf_phys = card.buffers.phys();
        for i in 0..RX_COUNT {
            let d = rx_descriptor(buf_phys + (i * BUFFER_SIZE) as u64);
            let _ = card.descriptors.write(i * 16, &d);
        }
        if pch {
            card.write(TDBAL, (desc_phys + TX_RING as u64) as u32);
            card.write(TDBAH, ((desc_phys + TX_RING as u64) >> 32) as u32);
            card.write(TDLEN, (TX_COUNT * 16) as u32);
            card.flush_rings(f);
        }
        // Interrupts, receive and transmit off; reset the MAC (not the
        // PHY, which keeps the link the firmware set up). 8254x SDM 14.5.
        card.write(IMC, 0xFFFF_FFFF);
        card.write(RCTL, 0);
        card.write(TCTL, TCTL_PSP);
        let _ = card.read(STATUS);
        spin_for(10_000_000, || false);
        card.write(CTRL, card.read(CTRL) | CTRL_RST);
        spin_for(20_000_000, || false);
        card.write(IMC, 0xFFFF_FFFF);
        let _ = card.read(ICR);
        // Link: autonegotiation decides speed and duplex.
        let ctrl = card.read(CTRL) & !(CTRL_FRCSPD | CTRL_FRCDPX);
        card.write(CTRL, ctrl | CTRL_SLU | CTRL_ASDE);
        card.mac = mac_from(card.read(RAL0), card.read(RAH0)).ok_or("no MAC address")?;
        for i in 0..128 {
            card.write(MTA + 4 * i, 0);
        }
        card.write(RDBAL, desc_phys as u32);
        card.write(RDBAH, (desc_phys >> 32) as u32);
        card.write(RDLEN, (RX_COUNT * 16) as u32);
        card.write(RDH, 0);
        card.write(RDT, (RX_COUNT - 1) as u32);
        let tx = desc_phys + TX_RING as u64;
        card.write(TDBAL, tx as u32);
        card.write(TDBAH, (tx >> 32) as u32);
        card.write(TDLEN, (TX_COUNT * 16) as u32);
        card.write(TDH, 0);
        card.write(TDT, 0);
        if matches!(family, Family::E1000e { .. }) {
            // Write-back threshold 1, granularity in descriptors (82574
            // datasheet 10.2.6.11; the values Linux and FreeBSD use).
            card.write(TXDCTL, 32 | 1 << 8 | 1 << 16 | 1 << 22 | 1 << 24);
        }
        // IPGT 10 (8254x copper; 8 on the 82574 and I21x), IPGR1 8,
        // IPGR2 6 (8254x SDM 13.4.34).
        let ipgt = if family == Family::E1000 { 10 } else { 8 };
        card.write(TIPG, ipgt | 8 << 10 | 6 << 20);
        card.write(TCTL, TCTL_EN | TCTL_PSP | TCTL_CT_COLD | TCTL_RTLC);
        if pch {
            // I219 errata (specification update 1.5.4.5, as in FreeBSD).
            card.write(IOSFPC, card.read(IOSFPC) | 0x0001_0000);
            card.write(TARC0, (card.read(TARC0) & !(3 << 28)) | 2 << 28);
        }
        // 2048-byte buffers (BSIZE 00), broadcasts, strip the CRC.
        card.write(RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);
        let card = Arc::new(card);
        let _ = CARD.set(card.clone());
        // The 82540EM has no MSI (QEMU's e1000 neither): then it is
        // polled.
        if pci::enable_msi(f, interrupt).is_ok() {
            card.msi.store(true, Ordering::Release);
            card.write(IMS, INTERRUPTS);
        }
        // Autonegotiation takes a moment.
        spin_for(1_000_000_000, || card.read(STATUS) & STATUS_LU != 0);
        Ok(card)
    }

    impl NetDevice for E1000 {
        fn name(&self) -> &'static str {
            self.name
        }

        fn mac(&self) -> [u8; 6] {
            self.mac
        }

        fn link_up(&self) -> bool {
            self.read(STATUS) & STATUS_LU != 0
        }

        fn interrupts(&self) -> bool {
            self.msi.load(Ordering::Acquire)
        }

        fn send(&self, frame: &[u8]) -> Result<(), NetError> {
            if frame.is_empty() || frame.len() > BUFFER_SIZE {
                return Err(NetError::BadLength);
            }
            let mut rings = self.rings.lock();
            let i = rings.tx_next;
            let at = TX_RING + i * 16;
            // The slot is free if never used or sent (DD).
            let free = || {
                let d = self.descriptor(at);
                d[11] == 0 || d[12] & STATUS_DD != 0
            };
            if !free() && !spin_for(100_000_000, free) {
                return Err(NetError::Busy);
            }
            let buffer = TX_BUFFERS + i * BUFFER_SIZE;
            let _ = self.buffers.write(buffer, frame);
            let d = tx_descriptor(self.buffers.phys() + buffer as u64, frame.len() as u16);
            let _ = self.descriptors.write(at, &d);
            rings.tx_next = (i + 1) % TX_COUNT;
            self.write(TDT, rings.tx_next as u32);
            Ok(())
        }

        fn receive(&self, buf: &mut [u8]) -> Option<usize> {
            let mut rings = self.rings.lock();
            loop {
                let i = rings.rx_next;
                let d = self.descriptor(i * 16);
                let done = rx_done(&d)?;
                let got = match done {
                    Ok(len) if len <= buf.len() => {
                        let _ = self.buffers.read(i * BUFFER_SIZE, &mut buf[..len]);
                        Some(len)
                    }
                    _ => None,
                };
                // Back to the card.
                let _ = self.descriptors.write(
                    i * 16,
                    &rx_descriptor(self.buffers.phys() + (i * BUFFER_SIZE) as u64),
                );
                self.write(RDT, i as u32);
                rings.rx_next = (i + 1) % RX_COUNT;
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
    fn transmit_descriptor() {
        let d = tx_descriptor(0x1234_5678_9000, 60);
        assert_eq!(&d[0..8], &0x1234_5678_9000u64.to_le_bytes());
        assert_eq!(&d[8..10], &[60, 0]);
        assert_eq!(d[11], 0x0B);
        assert_eq!(d[12], 0);
    }

    #[test]
    fn receive_descriptor_states() {
        let mut d = rx_descriptor(0x8000);
        assert_eq!(rx_done(&d), None);
        d[8..10].copy_from_slice(&64u16.to_le_bytes());
        d[12] = STATUS_DD | STATUS_EOP;
        assert_eq!(rx_done(&d), Some(Ok(64)));
        d[13] = 0x80; // receive data error
        assert_eq!(rx_done(&d), Some(Err(())));
        d[13] = 0;
        d[12] = STATUS_DD; // not the end of the packet
        assert_eq!(rx_done(&d), Some(Err(())));
    }

    #[test]
    fn mac_address() {
        assert_eq!(
            mac_from(0x1200_5452, 0x8000_5634),
            Some([0x52, 0x54, 0x00, 0x12, 0x34, 0x56])
        );
        assert_eq!(mac_from(0x1200_5452, 0x5634), None);
    }
}
