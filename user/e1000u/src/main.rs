// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! e1000u (P13 K12, `docs/devices.md`): the kernel's e1000 driver
//! (`drivers/src/net/e1000.rs`) rewritten as a user program on the Device
//! interface. It finds an 82540EM (QEMU's e1000) or an 82574 (e1000e) on
//! bus 0 that no kernel driver took (boot option `usernet`), resets and
//! starts it with descriptor rings in DMA memory, attaches itself as the
//! kernel's network card (`NetTap`) and then moves frames: received ones
//! to the kernel (`NetDeliver`), the kernel's to the card (`NetTake`),
//! woken by the card's MSI (`DEVICE_IRQ`) or, without MSI, every 5 ms.
//!
//! Registers and descriptors: the 8254x Software Developer's Manual
//! (sections 3.2-3.4, 13) as in the kernel driver.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

use abi::{Rights, tag};
use alloc::format;
use alloc::string::String;
use libstela::device::{Bar, Device, NetTap};
use libstela::ipc::{Mapping, handle_duplicate, port_receive};
use libstela::{Message, bootstrap, println};

libstela::entry!(main);

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
const MTA: usize = 0x5200;
const RAL0: usize = 0x5400;
const RAH0: usize = 0x5404;

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
const TCTL_CT_COLD: u32 = 0x0F << 4 | 0x40 << 12;
const TCTL_RTLC: u32 = 1 << 24;
/// Link change, receive minimum threshold, overrun, receive timer.
const INTERRUPTS: u32 = 1 << 2 | 1 << 4 | 1 << 6 | 1 << 7;

const RX_COUNT: usize = 32;
const TX_COUNT: usize = 8;
const BUFFER: usize = 2048;
/// The descriptor page: receive ring, then transmit ring.
const TX_RING: usize = RX_COUNT * 16;
/// Buffers: receive, then transmit.
const TX_BUFFERS: usize = RX_COUNT * BUFFER;

const STATUS_DD: u8 = 0x01;
const STATUS_EOP: u8 = 0x02;
/// End of packet, insert FCS, report status.
const TX_CMD: u8 = 0x01 | 0x02 | 0x08;

/// The supported chips: (device id, name, is it an 82574).
const CHIPS: [(u16, &str, bool); 2] = [(0x100E, "e1000", false), (0x10D3, "e1000e", true)];

struct Card {
    regs: Bar,
    descriptors: Mapping,
    buffers: Mapping,
    buf_phys: u64,
    rx_next: usize,
    tx_next: usize,
}

fn spin_ms(ms: u64) {
    libstela::sleep_ms(ms);
}

impl Card {
    fn descriptor(&self, offset: usize) -> [u8; 16] {
        let mut d = [0u8; 16];
        let _ = self.descriptors.read(offset as u64, &mut d);
        d
    }

    fn rx_descriptor(&self, i: usize) -> [u8; 16] {
        let mut d = [0u8; 16];
        d[..8].copy_from_slice(&(self.buf_phys + (i * BUFFER) as u64).to_le_bytes());
        d
    }

    /// The next received frame into `buf`, if any.
    fn receive(&mut self, buf: &mut [u8]) -> Option<usize> {
        loop {
            let i = self.rx_next;
            let d = self.descriptor(i * 16);
            if d[12] & STATUS_DD == 0 {
                return None;
            }
            let len = usize::from(u16::from_le_bytes([d[8], d[9]]));
            let good = d[12] & STATUS_EOP != 0 && d[13] == 0 && len <= buf.len();
            if good {
                let _ = self.buffers.read((i * BUFFER) as u64, &mut buf[..len]);
            }
            let fresh = self.rx_descriptor(i);
            let _ = self.descriptors.write((i * 16) as u64, &fresh);
            self.regs.write_u32(RDT, i as u32);
            self.rx_next = (i + 1) % RX_COUNT;
            if good {
                return Some(len);
            }
        }
    }

    /// Queues `frame`; false if the ring stayed full.
    fn send(&mut self, frame: &[u8]) -> bool {
        let i = self.tx_next;
        let at = TX_RING + i * 16;
        for _ in 0..100 {
            let d = self.descriptor(at);
            if d[11] == 0 || d[12] & STATUS_DD != 0 {
                let buffer = TX_BUFFERS + i * BUFFER;
                let _ = self.buffers.write(buffer as u64, frame);
                let mut d = [0u8; 16];
                d[..8].copy_from_slice(&(self.buf_phys + buffer as u64).to_le_bytes());
                d[8..10].copy_from_slice(&(frame.len() as u16).to_le_bytes());
                d[11] = TX_CMD;
                let _ = self.descriptors.write(at as u64, &d);
                self.tx_next = (i + 1) % TX_COUNT;
                self.regs.write_u32(TDT, self.tx_next as u32);
                return true;
            }
            spin_ms(1);
        }
        false
    }
}

/// The first supported chip on bus 0 that the kernel lent us.
fn find() -> Option<(Device, String, &'static str, bool)> {
    for dev in 0..32u8 {
        let location = format!("00:{dev:02x}.0");
        let Ok(d) = Device::open(&location) else {
            continue;
        };
        let ids = d.config_read(0, 4).unwrap_or(0);
        let (vendor, device) = (ids as u16, (ids >> 16) as u16);
        if vendor != 0x8086 {
            continue;
        }
        if let Some(&(_, name, e)) = CHIPS.iter().find(|c| c.0 == device) {
            return Some((d, location, name, e));
        }
    }
    None
}

/// Resets and starts the card (the kernel driver's order).
fn start(d: &Device, e1000e: bool) -> Result<(Card, [u8; 6]), &'static str> {
    let regs = d.map_bar(0).map_err(|_| "cannot map BAR0")?;
    let (descriptors, desc_phys) = d.dma(1).map_err(|_| "no DMA memory")?;
    let pages = (RX_COUNT + TX_COUNT) * BUFFER / 4096;
    let (buffers, buf_phys) = d.dma(pages).map_err(|_| "no DMA memory")?;
    let card = Card {
        regs,
        descriptors,
        buffers,
        buf_phys,
        rx_next: 0,
        tx_next: 0,
    };
    for i in 0..RX_COUNT {
        let rd = card.rx_descriptor(i);
        let _ = card.descriptors.write((i * 16) as u64, &rd);
    }
    let r = &card.regs;
    r.write_u32(IMC, 0xFFFF_FFFF);
    r.write_u32(RCTL, 0);
    r.write_u32(TCTL, TCTL_PSP);
    let _ = r.read_u32(STATUS);
    spin_ms(10);
    r.write_u32(CTRL, r.read_u32(CTRL) | CTRL_RST);
    spin_ms(20);
    r.write_u32(IMC, 0xFFFF_FFFF);
    let _ = r.read_u32(ICR);
    let ctrl = r.read_u32(CTRL) & !(CTRL_FRCSPD | CTRL_FRCDPX);
    r.write_u32(CTRL, ctrl | CTRL_SLU | CTRL_ASDE);
    let (ral, rah) = (r.read_u32(RAL0), r.read_u32(RAH0));
    if rah & (1 << 31) == 0 {
        return Err("no MAC address");
    }
    let (l, h) = (ral.to_le_bytes(), rah.to_le_bytes());
    let mac = [l[0], l[1], l[2], l[3], h[0], h[1]];
    for i in 0..128 {
        r.write_u32(MTA + 4 * i, 0);
    }
    r.write_u32(RDBAL, desc_phys as u32);
    r.write_u32(RDBAH, (desc_phys >> 32) as u32);
    r.write_u32(RDLEN, (RX_COUNT * 16) as u32);
    r.write_u32(RDH, 0);
    r.write_u32(RDT, (RX_COUNT - 1) as u32);
    let tx = desc_phys + TX_RING as u64;
    r.write_u32(TDBAL, tx as u32);
    r.write_u32(TDBAH, (tx >> 32) as u32);
    r.write_u32(TDLEN, (TX_COUNT * 16) as u32);
    r.write_u32(TDH, 0);
    r.write_u32(TDT, 0);
    if e1000e {
        r.write_u32(TXDCTL, 32 | 1 << 8 | 1 << 16 | 1 << 22 | 1 << 24);
    }
    let ipgt = if e1000e { 8 } else { 10 };
    r.write_u32(TIPG, ipgt | 8 << 10 | 6 << 20);
    r.write_u32(TCTL, TCTL_EN | TCTL_PSP | TCTL_CT_COLD | TCTL_RTLC);
    r.write_u32(RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);
    for _ in 0..100 {
        if r.read_u32(STATUS) & STATUS_LU != 0 {
            break;
        }
        spin_ms(10);
    }
    Ok((card, mac))
}

fn main() -> i64 {
    let Some((device, location, name, e1000e)) = find() else {
        println!("e1000u: no free e1000 on bus 0 (boot option usernet?)");
        return 1;
    };
    let (mut card, mac) = match start(&device, e1000e) {
        Ok(c) => c,
        Err(why) => {
            println!("e1000u: {location}: {why}");
            return 1;
        }
    };
    // Interrupts and the kernel's frames come to the main port.
    let Ok(port) = handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND | Rights::TRANSFER) else {
        return 1;
    };
    let msi = match device.irq(port) {
        Ok(v) => {
            card.regs.write_u32(IMS, INTERRUPTS);
            Some(v)
        }
        Err(_) => None,
    };
    let Ok(port) = handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND | Rights::TRANSFER) else {
        return 1;
    };
    let tap = match NetTap::attach(port, mac) {
        Ok(t) => t,
        Err(e) => {
            println!("e1000u: cannot attach to the network: {}", e.name());
            return 1;
        }
    };
    println!(
        "e1000u: {location} {name} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, {}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        match msi {
            Some(v) => format!("MSI vector {v}"),
            None => String::from("polled"),
        }
    );
    let mut frame = [0u8; BUFFER];
    let mut msg = Message::new();
    let mut irqs = 0u64;
    loop {
        // Woken by an interrupt, frames to send, or the poll interval.
        let timeout = if msi.is_some() {
            100_000_000
        } else {
            5_000_000
        };
        if port_receive(bootstrap::MAIN_PORT, &mut msg, timeout).is_ok() {
            if msg.tag == tag::DEVICE_IRQ {
                // Reading ICR acknowledges every cause.
                let _ = card.regs.read_u32(ICR);
                irqs += 1;
                if irqs == 1 {
                    println!("e1000u: first interrupt");
                }
            }
            if msg.tag != tag::DEVICE_IRQ && msg.tag != tag::NET_TX_READY {
                for &h in msg.carried() {
                    let _ = libstela::ipc::handle_close(h);
                }
            }
        }
        while let Some(n) = card.receive(&mut frame) {
            let _ = tap.deliver(&frame[..n]);
        }
        while let Ok(n) = tap.take(&mut frame) {
            if n == 0 || !card.send(&frame[..n]) {
                break;
            }
        }
    }
}
