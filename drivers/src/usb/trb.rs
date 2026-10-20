// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! xHCI Transfer Request Blocks and rings (eXtensible Host Controller
//! Interface for USB 1.2, 4.9 "TRB Ring", 6.4 "Transfer Request Block").
//! Pure logic, tested on the host.
//!
//! ```text
//!  TRB (16 bytes):  parameter u64 | status u32 | control u32
//!  control:  bit 0 cycle, bit 1 ENT/TC, bit 5 IOC, bit 6 IDT,
//!            bits 15:10 type, bits 31:24 slot id (commands, events)
//! ```
//!
//! A ring is an array of TRBs whose last entry is a Link TRB back to the
//! start. The producer writes TRBs with its cycle bit; the consumer takes
//! the ones whose cycle bit matches its own and flips its bit when it
//! wraps. The Link TRB's "toggle cycle" flag tells the controller to flip
//! too.

/// One TRB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trb {
    /// Pointer or immediate data.
    pub parameter: u64,
    /// Lengths, completion code.
    pub status: u32,
    /// Cycle, flags, type.
    pub control: u32,
}

/// TRB types (xHCI 1.2 Table 6-91).
pub mod kind {
    #![allow(missing_docs)]
    pub const NORMAL: u32 = 1;
    pub const SETUP: u32 = 2;
    pub const DATA: u32 = 3;
    pub const STATUS: u32 = 4;
    pub const LINK: u32 = 6;
    pub const ENABLE_SLOT: u32 = 9;
    pub const DISABLE_SLOT: u32 = 10;
    pub const ADDRESS_DEVICE: u32 = 11;
    pub const CONFIGURE_ENDPOINT: u32 = 12;
    pub const EVALUATE_CONTEXT: u32 = 13;
    pub const NO_OP_COMMAND: u32 = 23;
    pub const TRANSFER_EVENT: u32 = 32;
    pub const COMMAND_COMPLETION: u32 = 33;
    pub const PORT_STATUS_CHANGE: u32 = 34;
}

/// Completion codes (xHCI 1.2 Table 6-90).
pub mod completion {
    #![allow(missing_docs)]
    pub const SUCCESS: u8 = 1;
    pub const SHORT_PACKET: u8 = 13;
}

const CYCLE: u32 = 1 << 0;
const TOGGLE_CYCLE: u32 = 1 << 1;
const ISP: u32 = 1 << 2;
const IOC: u32 = 1 << 5;
const IDT: u32 = 1 << 6;
/// Data/Status stage direction: IN.
const DIR_IN: u32 = 1 << 16;

impl Trb {
    /// The TRB type.
    pub const fn kind(&self) -> u32 {
        (self.control >> 10) & 0x3F
    }

    /// The cycle bit.
    pub const fn cycle(&self) -> bool {
        self.control & CYCLE != 0
    }

    /// An event's completion code.
    pub const fn completion(&self) -> u8 {
        (self.status >> 24) as u8
    }

    /// The slot id of a command or event.
    pub const fn slot(&self) -> u8 {
        (self.control >> 24) as u8
    }

    /// A transfer event's endpoint (DCI).
    pub const fn endpoint(&self) -> u8 {
        ((self.control >> 16) & 0x1F) as u8
    }

    /// A transfer event's untransferred byte count.
    pub const fn residual(&self) -> u32 {
        self.status & 0x00FF_FFFF
    }

    /// A port status change event's port number.
    pub const fn port(&self) -> u8 {
        (self.parameter >> 24) as u8
    }

    /// The 16 bytes.
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&self.parameter.to_le_bytes());
        b[8..12].copy_from_slice(&self.status.to_le_bytes());
        b[12..].copy_from_slice(&self.control.to_le_bytes());
        b
    }

    /// From 16 bytes.
    pub fn from_bytes(b: &[u8; 16]) -> Trb {
        let mut p = [0u8; 8];
        p.copy_from_slice(&b[..8]);
        Trb {
            parameter: u64::from_le_bytes(p),
            status: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            control: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        }
    }

    const fn with(kind: u32) -> Trb {
        Trb {
            parameter: 0,
            status: 0,
            control: kind << 10,
        }
    }

    /// A command for `slot`.
    pub const fn command(kind: u32, slot: u8, parameter: u64) -> Trb {
        let mut t = Trb::with(kind);
        t.control |= (slot as u32) << 24;
        t.parameter = parameter;
        t
    }

    /// A Link TRB to `target` that toggles the cycle.
    pub const fn link(target: u64) -> Trb {
        let mut t = Trb::with(kind::LINK);
        t.parameter = target;
        t.control |= TOGGLE_CYCLE;
        t
    }

    /// A Setup Stage TRB (xHCI 1.2 6.4.1.2.1): immediate data; transfer
    /// type 0 no data, 2 OUT, 3 IN.
    pub const fn setup(packet: u64, length: u16, data_in: bool) -> Trb {
        let mut t = Trb::with(kind::SETUP);
        t.parameter = packet;
        t.status = 8;
        let transfer = if length == 0 {
            0
        } else if data_in {
            3
        } else {
            2
        };
        t.control |= IDT | transfer << 16;
        t
    }

    /// A Data Stage TRB.
    pub const fn data(buffer: u64, length: u16, data_in: bool) -> Trb {
        let mut t = Trb::with(kind::DATA);
        t.parameter = buffer;
        t.status = length as u32;
        if data_in {
            t.control |= DIR_IN | ISP;
        }
        t
    }

    /// A Status Stage TRB: IN unless the data stage was IN (USB 2.0
    /// 8.5.3: the status stage goes the other way; with no data, IN).
    pub const fn status(data_in: bool, has_data: bool) -> Trb {
        let mut t = Trb::with(kind::STATUS);
        if !(has_data && data_in) {
            t.control |= DIR_IN;
        }
        t.control |= IOC;
        t
    }

    /// A Normal TRB for `length` bytes at `buffer`, interrupting on
    /// completion and on short packets.
    pub const fn normal(buffer: u64, length: u32) -> Trb {
        let mut t = Trb::with(kind::NORMAL);
        t.parameter = buffer;
        t.status = length & 0x1_FFFF;
        t.control |= IOC | ISP;
        t
    }

    /// This TRB with the producer's cycle bit.
    pub const fn with_cycle(mut self, cycle: bool) -> Trb {
        self.control = (self.control & !CYCLE) | if cycle { CYCLE } else { 0 };
        self
    }
}

/// The producer's position in a ring of `size` TRBs (the last is the
/// Link TRB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Producer {
    size: usize,
    /// Next TRB to write.
    pub index: usize,
    /// Its cycle bit.
    pub cycle: bool,
}

impl Producer {
    /// At the start of a fresh ring (cycle 1).
    pub const fn new(size: usize) -> Producer {
        Producer {
            size,
            index: 0,
            cycle: true,
        }
    }

    /// Takes the next slot: (index to write, cycle to write it with,
    /// whether the Link TRB must now be handed over with the old cycle
    /// before the wrap).
    pub fn advance(&mut self) -> (usize, bool, Option<bool>) {
        let at = self.index;
        let cycle = self.cycle;
        self.index += 1;
        let mut link = None;
        if self.index == self.size - 1 {
            link = Some(self.cycle);
            self.index = 0;
            self.cycle = !self.cycle;
        }
        (at, cycle, link)
    }
}

/// The consumer's position in the event ring (no Link TRB: the event ring
/// segment table gives its size).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Consumer {
    size: usize,
    /// Next TRB to look at.
    pub index: usize,
    /// The cycle bit a new TRB has.
    pub cycle: bool,
}

impl Consumer {
    /// At the start (cycle 1).
    pub const fn new(size: usize) -> Consumer {
        Consumer {
            size,
            index: 0,
            cycle: true,
        }
    }

    /// Whether `trb` (at `index`) is new.
    pub const fn is_new(&self, trb: &Trb) -> bool {
        trb.cycle() == self.cycle
    }

    /// Moves past the current TRB.
    pub fn advance(&mut self) {
        self.index += 1;
        if self.index == self.size {
            self.index = 0;
            self.cycle = !self.cycle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings() {
        let t = Trb::command(kind::ADDRESS_DEVICE, 3, 0x1000);
        assert_eq!(t.kind(), kind::ADDRESS_DEVICE);
        assert_eq!(t.slot(), 3);
        assert_eq!(t.to_bytes()[..8], 0x1000u64.to_le_bytes());
        assert_eq!(Trb::from_bytes(&t.to_bytes()), t);
        let s = Trb::setup(0x0012_0000_0100_0680, 18, true);
        assert_eq!(s.status, 8);
        assert_eq!(s.control, (2 << 10) | IDT | (3 << 16));
        assert_eq!(Trb::setup(0, 0, false).control, (2 << 10) | IDT);
        let d = Trb::data(0x2000, 18, true);
        assert_eq!(d.control & DIR_IN, DIR_IN);
        assert_eq!(Trb::status(true, true).control & DIR_IN, 0);
        assert_eq!(Trb::status(false, false).control & DIR_IN, DIR_IN);
        let l = Trb::link(0x4000).with_cycle(true);
        assert_eq!(l.control, (6 << 10) | TOGGLE_CYCLE | CYCLE);
        let n = Trb::normal(0x5000, 8);
        assert_eq!((n.kind(), n.status), (kind::NORMAL, 8));
    }

    #[test]
    fn events() {
        let e = Trb {
            parameter: 0x9000,
            status: (13 << 24) | 5,
            control: (7 << 24) | (3 << 16) | (32 << 10) | 1,
        };
        assert_eq!(
            (e.kind(), e.slot(), e.endpoint()),
            (kind::TRANSFER_EVENT, 7, 3)
        );
        assert_eq!(
            (e.completion(), e.residual()),
            (completion::SHORT_PACKET, 5)
        );
        let p = Trb {
            parameter: 2 << 24,
            status: 1 << 24,
            control: 34 << 10,
        };
        assert_eq!(p.port(), 2);
    }

    #[test]
    fn producer_wraps_through_the_link() {
        let mut p = Producer::new(4);
        assert_eq!(p.advance(), (0, true, None));
        assert_eq!(p.advance(), (1, true, None));
        // Index 2 is the last usable one; then the link (index 3) gets the
        // old cycle and the producer starts over with the other one.
        assert_eq!(p.advance(), (2, true, Some(true)));
        assert_eq!((p.index, p.cycle), (0, false));
        assert_eq!(p.advance(), (0, false, None));
    }

    #[test]
    fn consumer_flips_at_the_end() {
        let mut c = Consumer::new(2);
        let fresh = Trb::default().with_cycle(true);
        assert!(c.is_new(&fresh));
        c.advance();
        c.advance();
        assert_eq!((c.index, c.cycle), (0, false));
        assert!(!c.is_new(&fresh));
    }
}
