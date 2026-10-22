// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! USB Ethernet (P13 K10, `docs/usb.md` §13): which kind a device is and
//! the framing each kind puts around an Ethernet frame. Pure logic, tested
//! on the host; `xhci.rs` moves the bytes.
//!
//! ```text
//!  CDC-ECM  (class 02/06)   one frame per bulk transfer, nothing around it
//!  CDC-NCM  (class 02/0D)   NTB16: "NCMH" header, "NCM0" index of datagrams
//!  RNDIS    (E0/01/03, 02/02/FF, EF/04/01)  44-byte PACKET_MSG header;
//!           control messages (INITIALIZE, QUERY, SET) on endpoint 0
//!  AX88179  (vendor 0B95)    8-byte TX header; RX: frames, then a list of
//!           per-frame headers and a 4-byte trailer saying where it is
//! ```
//!
//! References: USB CDC 1.2 and its ECM 1.2 and NCM 1.0 subclass
//! specifications; Microsoft "Remote NDIS Specification" (RNDIS 1.0);
//! for the AX88179 only the register values and order of Linux's
//! `ax88179_178a.c` (no code), untested on real hardware.

use super::{Endpoint, descriptor};
use alloc::vec::Vec;

/// The kinds of USB network adapters STELA drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// CDC Ethernet Control Model.
    Ecm,
    /// CDC Network Control Model (16-bit NTBs).
    Ncm,
    /// Microsoft Remote NDIS.
    Rndis,
    /// ASIX AX88179 / AX88178A (USB 3 gigabit).
    Ax88179,
}

impl Kind {
    /// The name the kernel shows (`NetDevice::name`).
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Ecm => "usb-ecm",
            Kind::Ncm => "usb-ncm",
            Kind::Rndis => "usb-rndis",
            Kind::Ax88179 => "ax88179",
        }
    }

    /// How much better than the others (a device that offers several
    /// configurations is set to the best).
    const fn rank(self) -> u8 {
        match self {
            Kind::Ax88179 => 4,
            Kind::Ncm => 3,
            Kind::Ecm => 2,
            Kind::Rndis => 1,
        }
    }
}

/// A network function found in a configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    /// The kind.
    pub kind: Kind,
    /// The communication (control) interface (RNDIS messages go to it).
    pub control: u8,
    /// The data interface and the alternate setting with the endpoints
    /// (ECM, NCM: 1; SET_INTERFACE selects it).
    pub data: u8,
    /// The alternate setting.
    pub alt: u8,
    /// The string descriptor with the MAC address (ECM, NCM), 0: none.
    pub mac_string: u8,
    /// Bulk IN.
    pub bulk_in: Endpoint,
    /// Bulk OUT.
    pub bulk_out: Endpoint,
}

impl Function {
    /// Whether this function is better than `other`.
    pub fn better_than(&self, other: &Function) -> bool {
        self.kind.rank() > other.kind.rank()
    }
}

/// `CS_INTERFACE` (CDC 1.2 Table 12).
const CS_INTERFACE: u8 = 0x24;
/// Functional descriptor subtypes: Union, Ethernet Networking.
const UNION: u8 = 0x06;
const ETHERNET: u8 = 0x0F;

/// Finds a network function in a whole configuration descriptor set.
/// `vendor` and `product` pick the vendor-specific chips.
pub fn find_function(config: &[u8], vendor: u16, product: u16) -> Option<Function> {
    // Every interface descriptor (with its alternate setting), its
    // endpoints, and the CDC functional descriptors seen.
    struct Alt {
        number: u8,
        alt: u8,
        class: (u8, u8, u8),
        endpoints: Vec<Endpoint>,
    }
    let mut alts: Vec<Alt> = Vec::new();
    let mut union_slave: Option<(u8, u8)> = None;
    let mut mac_string = 0;
    let mut at = 0;
    while at + 2 <= config.len() {
        let len = config[at] as usize;
        if len < 2 || at + len > config.len() {
            break;
        }
        let d = &config[at..at + len];
        match d[1] {
            descriptor::INTERFACE if len >= 9 => alts.push(Alt {
                number: d[2],
                alt: d[3],
                class: (d[5], d[6], d[7]),
                endpoints: Vec::new(),
            }),
            descriptor::ENDPOINT if len >= 7 => {
                if let Some(a) = alts.last_mut() {
                    a.endpoints.push(Endpoint {
                        address: d[2],
                        kind: d[3] & 3,
                        max_packet: u16::from_le_bytes([d[4], d[5]]) & 0x7FF,
                        interval: d[6],
                    });
                }
            }
            CS_INTERFACE if len >= 5 && d[2] == UNION => union_slave = Some((d[3], d[4])),
            CS_INTERFACE if len >= 4 && d[2] == ETHERNET => mac_string = d[3],
            _ => {}
        }
        at += len;
    }
    let bulk = |a: &Alt| {
        let i = a
            .endpoints
            .iter()
            .find(|e| e.kind == 2 && e.is_in())
            .copied()?;
        let o = a
            .endpoints
            .iter()
            .find(|e| e.kind == 2 && !e.is_in())
            .copied()?;
        Some((i, o))
    };
    // The ASIX chips: one vendor-specific interface with both bulk pipes.
    if vendor == 0x0B95 && matches!(product, 0x1790 | 0x178A) {
        let a = alts.iter().find(|a| bulk(a).is_some())?;
        let (bulk_in, bulk_out) = bulk(a)?;
        return Some(Function {
            kind: Kind::Ax88179,
            control: a.number,
            data: a.number,
            alt: a.alt,
            mac_string: 0,
            bulk_in,
            bulk_out,
        });
    }
    let control = alts.iter().find(|a| a.alt == 0 && is_control(a.class))?;
    let kind = match control.class {
        (0x02, 0x06, _) => Kind::Ecm,
        (0x02, 0x0D, _) => Kind::Ncm,
        _ => Kind::Rndis,
    };
    // The data interface: the union's slave, else the next one.
    let data_number = union_slave
        .filter(|(m, _)| *m == control.number)
        .map_or(control.number + 1, |(_, s)| s);
    let data = alts
        .iter()
        .filter(|a| a.number == data_number && a.class.0 == 0x0A)
        .find(|a| bulk(a).is_some())?;
    let (bulk_in, bulk_out) = bulk(data)?;
    Some(Function {
        kind,
        control: control.number,
        data: data.number,
        alt: data.alt,
        mac_string: if kind == Kind::Rndis { 0 } else { mac_string },
        bulk_in,
        bulk_out,
    })
}

/// Whether an interface class is a network control interface.
fn is_control(class: (u8, u8, u8)) -> bool {
    matches!(
        class,
        (0x02, 0x06, _)
            | (0x02, 0x0D, _)
            | (0xE0, 0x01, 0x03)
            | (0x02, 0x02, 0xFF)
            | (0xEF, 0x04, 0x01)
    )
}

/// The MAC address in an ECM/NCM `iMACAddress` string descriptor: 12
/// hexadecimal digits in UTF-16LE after the 2-byte header.
pub fn mac_from_string(d: &[u8]) -> Option<[u8; 6]> {
    if d.len() < 2 + 24 || d[1] != 3 {
        return None;
    }
    let mut mac = [0u8; 6];
    for i in 0..12 {
        let c = char::from(d[2 + 2 * i]);
        let v = c.to_digit(16)? as u8;
        mac[i / 2] = (mac[i / 2] << 4) | v;
    }
    Some(mac)
}

/// RNDIS control and data messages (RNDIS 1.0 §2, §3).
pub mod rndis {
    use alloc::vec::Vec;

    /// `REMOTE_NDIS_PACKET_MSG`.
    pub const PACKET_MSG: u32 = 0x0000_0001;
    /// `REMOTE_NDIS_INITIALIZE_MSG` and its completion.
    pub const INITIALIZE_MSG: u32 = 0x0000_0002;
    /// `REMOTE_NDIS_INITIALIZE_CMPLT`.
    pub const INITIALIZE_CMPLT: u32 = 0x8000_0002;
    /// `REMOTE_NDIS_QUERY_MSG`.
    pub const QUERY_MSG: u32 = 0x0000_0004;
    /// `REMOTE_NDIS_QUERY_CMPLT`.
    pub const QUERY_CMPLT: u32 = 0x8000_0004;
    /// `REMOTE_NDIS_SET_MSG`.
    pub const SET_MSG: u32 = 0x0000_0005;
    /// `REMOTE_NDIS_SET_CMPLT`.
    pub const SET_CMPLT: u32 = 0x8000_0005;
    /// `OID_802_3_PERMANENT_ADDRESS`.
    pub const OID_PERMANENT_ADDRESS: u32 = 0x0101_0101;
    /// `OID_GEN_CURRENT_PACKET_FILTER`.
    pub const OID_PACKET_FILTER: u32 = 0x0001_010E;
    /// Directed, multicast, broadcast (`NDIS_PACKET_TYPE_*`).
    pub const FILTER: u32 = 0x1 | 0x2 | 0x8;
    /// Size of a `PACKET_MSG` header.
    pub const PACKET_HEADER: usize = 44;
    /// The largest transfer we ask the device for.
    pub const MAX_TRANSFER: u32 = 2048;

    fn words(ws: &[u32]) -> Vec<u8> {
        ws.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn word(b: &[u8], i: usize) -> Option<u32> {
        let at = i * 4;
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
    }

    /// `INITIALIZE_MSG` with request id `id`: version 1.0, our largest
    /// transfer.
    pub fn initialize(id: u32) -> Vec<u8> {
        words(&[INITIALIZE_MSG, 24, id, 1, 0, MAX_TRANSFER])
    }

    /// `QUERY_MSG` for `oid`, with a 4-byte input buffer of zeros (some
    /// devices, QEMU's among them, refuse an empty one whose offset is the
    /// end of the message).
    pub fn query(id: u32, oid: u32) -> Vec<u8> {
        words(&[QUERY_MSG, 32, id, oid, 4, 20, 0, 0])
    }

    /// `SET_MSG` of `oid` to the 4-byte `value`.
    pub fn set(id: u32, oid: u32, value: u32) -> Vec<u8> {
        words(&[SET_MSG, 32, id, oid, 4, 20, 0, value])
    }

    /// Whether `reply` completes request `id` of `kind` with success; the
    /// information buffer of a `QUERY_CMPLT`.
    pub fn completion(reply: &[u8], kind: u32, id: u32) -> Option<&[u8]> {
        if word(reply, 0)? != kind || word(reply, 2)? != id || word(reply, 3)? != 0 {
            return None;
        }
        if kind != QUERY_CMPLT {
            return Some(&[]);
        }
        let len = word(reply, 4)? as usize;
        let off = word(reply, 5)? as usize + 8;
        reply.get(off..off.checked_add(len)?)
    }

    /// An Ethernet frame inside a `PACKET_MSG`.
    pub fn wrap(frame: &[u8]) -> Vec<u8> {
        let mut m = words(&[
            PACKET_MSG,
            (PACKET_HEADER + frame.len()) as u32,
            36,
            frame.len() as u32,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ]);
        m.extend_from_slice(frame);
        m
    }

    /// The frames in a transfer of `PACKET_MSG`s.
    pub fn unwrap(mut transfer: &[u8]) -> Vec<&[u8]> {
        let mut frames = Vec::new();
        while let (Some(kind), Some(len)) = (word(transfer, 0), word(transfer, 1)) {
            let len = len as usize;
            if kind != PACKET_MSG || len < PACKET_HEADER || len > transfer.len() {
                break;
            }
            let (Some(off), Some(n)) = (word(transfer, 2), word(transfer, 3)) else {
                break;
            };
            let start = 8 + off as usize;
            if let Some(f) = transfer.get(start..start + n as usize)
                && start + n as usize <= len
            {
                frames.push(f);
            }
            transfer = &transfer[len..];
        }
        frames
    }
}

/// CDC-NCM 16-bit NTBs (NCM 1.0 §3.2, §3.3).
pub mod ncm {
    use alloc::vec::Vec;

    /// "NCMH".
    const NTH16: u32 = 0x484D_434E;
    /// "NCM0" (no CRC).
    const NDP16: u32 = 0x304D_434E;

    fn u16_at(b: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
    }

    /// One frame as an NTB16 with sequence number `seq`: header, the
    /// datagram pointer table, the frame (4-byte aligned).
    pub fn wrap(frame: &[u8], seq: u16) -> Vec<u8> {
        // NTH16 (12) | NDP16 (16: header 8, one entry 4, terminator 4) | frame
        let ndp = 12usize;
        let data = 28usize;
        let total = data + frame.len();
        let mut b = Vec::with_capacity(total);
        b.extend_from_slice(&NTH16.to_le_bytes());
        b.extend_from_slice(&12u16.to_le_bytes());
        b.extend_from_slice(&seq.to_le_bytes());
        b.extend_from_slice(&(total as u16).to_le_bytes());
        b.extend_from_slice(&(ndp as u16).to_le_bytes());
        b.extend_from_slice(&NDP16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // next NDP
        b.extend_from_slice(&(data as u16).to_le_bytes());
        b.extend_from_slice(&(frame.len() as u16).to_le_bytes());
        b.extend_from_slice(&[0u8; 4]);
        b.extend_from_slice(frame);
        b
    }

    /// The datagrams of a received NTB16.
    pub fn unwrap(ntb: &[u8]) -> Vec<&[u8]> {
        let mut frames = Vec::new();
        if ntb.get(0..4) != Some(&NTH16.to_le_bytes()) {
            return frames;
        }
        let Some(mut ndp) = u16_at(ntb, 10).map(usize::from) else {
            return frames;
        };
        // Follow the chain of NDPs (at most a few).
        for _ in 0..8 {
            if ndp == 0 || ntb.get(ndp..ndp + 4).is_none_or(|s| s[..3] != *b"NCM") {
                break;
            }
            let len = u16_at(ntb, ndp + 4).map_or(0, usize::from);
            let next = u16_at(ntb, ndp + 6).map_or(0, usize::from);
            let mut e = ndp + 8;
            while e + 4 <= ndp + len {
                let (Some(off), Some(n)) = (u16_at(ntb, e), u16_at(ntb, e + 2)) else {
                    break;
                };
                if off == 0 || n == 0 {
                    break;
                }
                if let Some(f) = ntb.get(usize::from(off)..usize::from(off) + usize::from(n)) {
                    frames.push(f);
                }
                e += 4;
            }
            ndp = next;
        }
        frames
    }
}

/// ASIX AX88179 framing (values from Linux's `ax88179_178a.c`).
pub mod ax88179 {
    use alloc::vec::Vec;

    /// Vendor requests: access a MAC register.
    pub const ACCESS_MAC: u8 = 0x01;
    /// Registers: node id (MAC), RX control, medium mode, PHY power and
    /// reset, clock select, monitor mode, bulk-in queue control.
    pub const NODE_ID: u16 = 0x10;
    /// RX control register.
    pub const RX_CTL: u16 = 0x0B;
    /// Medium status mode.
    pub const MEDIUM_STATUS_MODE: u16 = 0x22;
    /// PHY power and reset control.
    pub const PHYPWR_RSTCTL: u16 = 0x26;
    /// Clock select.
    pub const CLK_SELECT: u16 = 0x33;
    /// Monitor mode.
    pub const MONITOR_MOD: u16 = 0x24;
    /// Bulk-in queue control.
    pub const RX_BULKIN_QCTRL: u16 = 0x2E;

    /// The register writes that bring the chip up, in order: (register,
    /// bytes). PHY on, clocks, bulk-in aggregation (high speed values),
    /// no checksum offload, monitor off, receive (directed, broadcast,
    /// multicast; drop CRC errors; header for IP alignment), medium
    /// (receive on, full duplex, gigabit, flow control).
    pub fn init_writes() -> Vec<(u16, Vec<u8>)> {
        alloc::vec![
            (PHYPWR_RSTCTL, alloc::vec![0x00, 0x00]),
            (PHYPWR_RSTCTL, alloc::vec![0x20, 0x00]),
            (CLK_SELECT, alloc::vec![0x03]),
            (RX_BULKIN_QCTRL, alloc::vec![0x07, 0x4F, 0x00, 0x12, 0xFF]),
            (0x34, alloc::vec![0x00]), // RXCOE_CTL
            (0x35, alloc::vec![0x00]), // TXCOE_CTL
            (MONITOR_MOD, alloc::vec![0x00]),
            (RX_CTL, alloc::vec![0x98, 0x03]),
            (MEDIUM_STATUS_MODE, alloc::vec![0x33, 0x01]),
        ]
    }

    /// One frame with its 8-byte TX header (length, then 0).
    pub fn wrap(frame: &[u8]) -> Vec<u8> {
        let mut b = Vec::with_capacity(8 + frame.len());
        b.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(frame);
        b
    }

    /// The frames of an RX transfer: the last 4 bytes give the number of
    /// packets and where their 4-byte headers start; each header's bits
    /// 16-28 are the length (frames start 8-byte aligned, after 2 bytes of
    /// padding for IP alignment); bit 31 or 29 marks a bad frame.
    pub fn unwrap(t: &[u8]) -> Vec<&[u8]> {
        let mut frames = Vec::new();
        if t.len() < 4 {
            return frames;
        }
        let tail = u32::from_le_bytes([
            t[t.len() - 4],
            t[t.len() - 3],
            t[t.len() - 2],
            t[t.len() - 1],
        ]);
        let count = (tail & 0xFFFF) as usize;
        let mut hdr = (tail >> 16) as usize;
        let mut at = 0usize;
        for _ in 0..count.min(256) {
            let Some(h) = t.get(hdr..hdr + 4) else {
                break;
            };
            let h = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
            let len = ((h >> 16) & 0x1FFF) as usize;
            let bad = h & (1 << 31) != 0 || h & (1 << 29) != 0;
            if !bad
                && len > 2
                && let Some(f) = t.get(at + 2..at + len)
            {
                frames.push(f);
            }
            at = (at + len + 7) & !7;
            hdr += 4;
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;

    /// QEMU's usb-net CDC Ethernet configuration (shortened): a
    /// communication interface 02/06 with header, union and Ethernet
    /// functional descriptors and an interrupt endpoint; a data interface
    /// with alternate setting 0 (no endpoints) and 1 (bulk IN 0x81 and OUT
    /// 0x02).
    fn ecm_config() -> std::vec::Vec<u8> {
        let mut c = vec![9, 2, 0, 0, 2, 1, 0, 0xC0, 50];
        c.extend_from_slice(&[9, 4, 0, 0, 1, 2, 6, 0, 0]);
        c.extend_from_slice(&[5, 0x24, 0, 0x10, 1]);
        c.extend_from_slice(&[5, 0x24, 6, 0, 1]);
        c.extend_from_slice(&[13, 0x24, 0x0F, 3, 0, 0, 0, 0, 0xEA, 5, 0, 0, 0]);
        c.extend_from_slice(&[7, 5, 0x83, 3, 16, 0, 8]);
        c.extend_from_slice(&[9, 4, 1, 0, 0, 0x0A, 0, 0, 0]);
        c.extend_from_slice(&[9, 4, 1, 1, 2, 0x0A, 0, 0, 0]);
        c.extend_from_slice(&[7, 5, 0x81, 2, 64, 0, 0]);
        c.extend_from_slice(&[7, 5, 0x02, 2, 64, 0, 0]);
        let len = c.len() as u16;
        c[2..4].copy_from_slice(&len.to_le_bytes());
        c
    }

    #[test]
    fn finds_ecm_and_its_data_interface() {
        let f = find_function(&ecm_config(), 0x0525, 0xA4A2).expect("an ECM function");
        assert_eq!(f.kind, Kind::Ecm);
        assert_eq!((f.control, f.data, f.alt, f.mac_string), (0, 1, 1, 3));
        assert_eq!((f.bulk_in.address, f.bulk_out.address), (0x81, 0x02));
        assert_eq!(f.kind.name(), "usb-ecm");
    }

    #[test]
    fn finds_rndis_and_ranks_kinds() {
        let mut c = ecm_config();
        // The control interface as RNDIS (E0/01/03) and the data
        // interface's endpoints in alternate setting 0.
        c[9 + 5..9 + 8].copy_from_slice(&[0xE0, 1, 3]);
        let f = find_function(&c, 0x0525, 0xA4A2).expect("an RNDIS function");
        assert_eq!(f.kind, Kind::Rndis);
        assert_eq!(f.mac_string, 0);
        let ecm = find_function(&ecm_config(), 0, 0).expect("ecm");
        assert!(ecm.better_than(&f));
        assert!(!f.better_than(&ecm));
        // Something else entirely.
        assert_eq!(find_function(&[9, 2, 9, 0, 0, 1, 0, 0, 0], 0, 0), None);
    }

    #[test]
    fn finds_the_asix_chip() {
        let mut c = vec![9, 2, 0, 0, 1, 1, 0, 0xC0, 50];
        c.extend_from_slice(&[9, 4, 0, 0, 3, 0xFF, 0xFF, 0, 0]);
        c.extend_from_slice(&[7, 5, 0x81, 3, 8, 0, 11]);
        c.extend_from_slice(&[7, 5, 0x82, 2, 0, 2, 0]);
        c.extend_from_slice(&[7, 5, 0x03, 2, 0, 2, 0]);
        let f = find_function(&c, 0x0B95, 0x1790).expect("ax88179");
        assert_eq!(f.kind, Kind::Ax88179);
        assert_eq!((f.bulk_in.address, f.bulk_out.address), (0x82, 0x03));
        assert_eq!(find_function(&c, 0x0B95, 0x7720), None);
    }

    #[test]
    fn mac_strings() {
        let mut d = vec![26, 3];
        for c in "525400123456".chars() {
            d.extend_from_slice(&[c as u8, 0]);
        }
        assert_eq!(mac_from_string(&d), Some([0x52, 0x54, 0, 0x12, 0x34, 0x56]));
        d[4] = b'x';
        assert_eq!(mac_from_string(&d), None);
        assert_eq!(mac_from_string(&[4, 3, b'5', 0]), None);
    }

    #[test]
    fn rndis_messages() {
        let m = rndis::initialize(1);
        assert_eq!(m.len(), 24);
        assert_eq!(&m[..4], &rndis::INITIALIZE_MSG.to_le_bytes());
        assert_eq!(rndis::query(2, rndis::OID_PERMANENT_ADDRESS).len(), 32);
        assert_eq!(
            rndis::set(3, rndis::OID_PACKET_FILTER, rndis::FILTER).len(),
            32
        );
        // A QUERY_CMPLT with a 6-byte answer at offset 16 (from byte 8).
        let mut q = std::vec::Vec::new();
        for w in [rndis::QUERY_CMPLT, 30, 2, 0, 6, 16] {
            q.extend_from_slice(&w.to_le_bytes());
        }
        q.extend_from_slice(&[0x52, 0x54, 0, 1, 2, 3]);
        assert_eq!(
            rndis::completion(&q, rndis::QUERY_CMPLT, 2),
            Some(&[0x52, 0x54, 0, 1, 2, 3][..])
        );
        assert_eq!(rndis::completion(&q, rndis::QUERY_CMPLT, 9), None);
        // Two packets in one transfer.
        let mut t = rndis::wrap(b"first frame");
        t.extend_from_slice(&rndis::wrap(b"second"));
        assert_eq!(rndis::unwrap(&t), vec![&b"first frame"[..], &b"second"[..]]);
        assert_eq!(rndis::wrap(b"x").len(), rndis::PACKET_HEADER + 1);
        assert!(rndis::unwrap(&t[..20]).is_empty());
    }

    #[test]
    fn ncm_blocks() {
        let b = ncm::wrap(b"an ethernet frame", 7);
        assert_eq!(&b[..4], b"NCMH");
        assert_eq!(ncm::unwrap(&b), vec![&b"an ethernet frame"[..]]);
        assert!(ncm::unwrap(b"garbage!").is_empty());
    }

    #[test]
    fn ax88179_framing() {
        let t = ax88179::wrap(b"frame");
        assert_eq!(&t[..4], &5u32.to_le_bytes());
        // RX: two frames (2 bytes of padding each, 8-byte aligned), their
        // headers at 24, the trailer.
        let mut r = std::vec::Vec::new();
        r.extend_from_slice(&[0, 0]);
        r.extend_from_slice(b"hello!");
        r.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        r.truncate(8);
        r.extend_from_slice(&[0, 0]);
        r.extend_from_slice(b"second");
        r.extend_from_slice(&[0; 8]);
        r.truncate(16);
        while r.len() < 24 {
            r.push(0);
        }
        r.extend_from_slice(&((8u32) << 16).to_le_bytes());
        r.extend_from_slice(&((8u32) << 16).to_le_bytes());
        r.extend_from_slice(&(2u32 | (24u32 << 16)).to_le_bytes());
        assert_eq!(ax88179::unwrap(&r), vec![&b"hello!"[..], &b"second"[..]]);
        assert!(ax88179::unwrap(&[1, 2]).is_empty());
        assert_eq!(ax88179::init_writes().len(), 9);
    }
}
