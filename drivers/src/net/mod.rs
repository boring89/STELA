// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Network cards (`docs/net.md`): Intel e1000 (82540EM, QEMU's default),
//! e1000e (82574L, QEMU `e1000e`; the I219 family of the P520) and
//! virtio-net (modern interface, VIRTIO 1.1). Each driver keeps a
//! receive and a transmit ring in DMA memory and moves whole Ethernet
//! frames; the protocols are the `net` crate's business.
//!
//! The parts that are pure logic (which card a PCI id is, descriptor
//! layouts, the virtio capability list and ring offsets) build on the
//! host and have tests; the drivers themselves are in [`e1000`] and
//! [`virtio`].

pub mod e1000;
pub mod virtio;

/// The drivers' families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// 8254x (82540EM and relatives): legacy descriptors, no MSI on the
    /// 82540EM.
    E1000,
    /// 82574L and the PCH LAN controllers (I217/I218/I219): the same
    /// legacy descriptors, MSI.
    E1000e {
        /// An I21x part (needs the PCH workarounds).
        pch: bool,
    },
    /// virtio-net, modern (PCI 1AF4:1041) or transitional (1AF4:1000)
    /// with the modern capabilities.
    Virtio,
}

/// The family of PCI device `vendor:device`, if a driver handles it.
pub fn family(vendor: u16, device: u16) -> Option<Family> {
    match (vendor, device) {
        // 82540EM (QEMU e1000), 82545EM, 82543GC, 82541PI.
        (0x8086, 0x100E | 0x100F | 0x1004 | 0x107C) => Some(Family::E1000),
        // 82574L (QEMU e1000e).
        (0x8086, 0x10D3) => Some(Family::E1000e { pch: false }),
        // I217, I218, I219 (LM and V variants; the P520 has 15B7).
        (
            0x8086,
            0x153A | 0x153B | 0x155A | 0x1559 | 0x15A0 | 0x15A1 | 0x15A2 | 0x15A3 | 0x156F | 0x1570
            | 0x15B7 | 0x15B8 | 0x15B9 | 0x15D7 | 0x15D8 | 0x15E3 | 0x15D6 | 0x15BD | 0x15BE
            | 0x15BB | 0x15BC,
        ) => Some(Family::E1000e { pch: true }),
        (0x1AF4, 0x1000 | 0x1041) => Some(Family::Virtio),
        _ => None,
    }
}

/// Size of every receive and transmit buffer.
pub const BUFFER_SIZE: usize = 2048;

#[cfg(target_os = "none")]
pub use system::*;

#[cfg(target_os = "none")]
mod system {
    use super::Family;
    use crate::pci::system as pci;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicU64, Ordering};
    use frame::sync::Once;

    /// Why a send failed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum NetError {
        /// The frame is empty or larger than a buffer.
        BadLength,
        /// The transmit ring stayed full.
        Busy,
    }

    /// A network card, as the kernel's network thread uses it.
    pub trait NetDevice: Send + Sync {
        /// "e1000", "e1000e" or "virtio-net".
        fn name(&self) -> &'static str;
        /// The card's MAC address.
        fn mac(&self) -> [u8; 6];
        /// Whether the link is up.
        fn link_up(&self) -> bool;
        /// Queues one Ethernet frame (without the CRC) for sending.
        fn send(&self, frame: &[u8]) -> Result<(), NetError>;
        /// Takes the next received frame into `buf`; its length, or `None`
        /// when the ring is empty.
        fn receive(&self, buf: &mut [u8]) -> Option<usize>;
        /// Whether the card interrupts (MSI); without, the caller polls.
        fn interrupts(&self) -> bool;
    }

    /// The thread the card's interrupt wakes (0: none).
    static WAITER: AtomicU64 = AtomicU64::new(0);
    /// The card in use (one at a time, `docs/net.md`).
    static CARD: Once<Arc<dyn NetDevice>> = Once::new();

    /// Makes the card's interrupts wake thread `id`.
    pub fn set_waiter(id: u64) {
        WAITER.store(id, Ordering::Release);
    }

    /// Wakes the waiting thread (from the drivers' interrupt handlers).
    pub(super) fn wake_waiter() {
        crate::sync::wake(WAITER.load(Ordering::Acquire));
    }

    /// The first card a driver handles, started; `None` if there is none.
    /// Call once at boot (after `block::probe`, which gives the drivers
    /// the scheduler hooks).
    pub fn probe() -> Option<Arc<dyn NetDevice>> {
        if let Some(card) = CARD.get() {
            return Some(card.clone());
        }
        for f in pci::devices() {
            let Some(family) = super::family(f.vendor, f.device) else {
                continue;
            };
            let card: Result<Arc<dyn NetDevice>, &'static str> = match family {
                Family::E1000 | Family::E1000e { .. } => {
                    super::e1000::init(f, family).map(|c| c as Arc<dyn NetDevice>)
                }
                Family::Virtio => super::virtio::init(f).map(|c| c as Arc<dyn NetDevice>),
            };
            match card {
                Ok(card) => {
                    let m = card.mac();
                    frame::kprintln!(
                        "net: {} {}: {} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, link {}{}",
                        card.name(),
                        f.address,
                        f.kind(),
                        m[0],
                        m[1],
                        m[2],
                        m[3],
                        m[4],
                        m[5],
                        if card.link_up() { "up" } else { "down" },
                        if card.interrupts() {
                            ", MSI"
                        } else {
                            ", polled"
                        }
                    );
                    let _ = CARD.set(card.clone());
                    return Some(card);
                }
                Err(why) => frame::kprintln!("net: {}: {why}", f.address),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        assert_eq!(family(0x8086, 0x100E), Some(Family::E1000));
        assert_eq!(family(0x8086, 0x10D3), Some(Family::E1000e { pch: false }));
        assert_eq!(family(0x8086, 0x15B7), Some(Family::E1000e { pch: true }));
        assert_eq!(family(0x1AF4, 0x1000), Some(Family::Virtio));
        assert_eq!(family(0x1AF4, 0x1041), Some(Family::Virtio));
        assert_eq!(family(0x1AF4, 0x1001), None);
        assert_eq!(family(0x10EC, 0x8139), None);
    }
}
