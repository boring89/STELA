// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! bsdnet (P14 K13-K14, `docs/bsdkpi.md`): FreeBSD's own `if_em` driver
//! (`sys/dev/e1000` of FreeBSD 14.3, unchanged) as a STELA user program.
//! A small FreeBSD KPI (`shim/`: bus_space on a mapped BAR, DMA memory,
//! callouts, a tiny iflib) runs it on the Device interface, like the
//! Rust driver e1000u; frames go to the kernel through a `NetTap`. init
//! starts it with the `DEVICES` capability for the boot option
//! `usernet=bsd`.
//!
//! One thread: the card's interrupts (MSI, `DEVICE_IRQ`) or a 5 ms poll,
//! the kernel's frames (`NET_TX_READY`) and the timers all come through
//! the main port.

#![no_std]
#![no_main]
#![deny(unsafe_code)]

extern crate alloc;

mod ffi;

use abi::{Rights, tag};
use alloc::format;
use alloc::vec::Vec;
use libstela::device::{Device, NetTap};
use libstela::ipc::{handle_duplicate, port_receive};
use libstela::{Message, bootstrap, println};

libstela::entry!(main);

/// Intel Ethernet functions on bus 0 (if_em's own table decides).
fn find() -> Option<(Device, alloc::string::String, (u16, u16, u16, u16, u8))> {
    for dev in 0..32u8 {
        let location = format!("00:{dev:02x}.0");
        let Ok(d) = Device::open(&location) else {
            continue;
        };
        let ids = d.config_read(0, 4).unwrap_or(0);
        let class = d.config_read(0x0B, 1).unwrap_or(0);
        if ids as u16 != 0x8086 || class != 0x02 {
            continue;
        }
        let sub = d.config_read(0x2C, 4).unwrap_or(0);
        let rev = d.config_read(0x08, 1).unwrap_or(0) as u8;
        let all = (
            ids as u16,
            (ids >> 16) as u16,
            sub as u16,
            (sub >> 16) as u16,
            rev,
        );
        return Some((d, location, all));
    }
    None
}

fn main() -> i64 {
    let Some((device, location, ids)) = find() else {
        println!("bsdnet: no free Intel Ethernet on bus 0 (boot option usernet=bsd?)");
        return 1;
    };
    println!(
        "bsdnet: {location} {:04x}:{:04x}, FreeBSD 14.3 if_em",
        ids.0, ids.1
    );
    *ffi::HOST.lock() = Some(ffi::Host {
        device,
        bars: Vec::new(),
        dma: Vec::new(),
        tap: None,
        received: 0,
    });
    let mac = match ffi::attach(ids) {
        Ok(m) => m,
        Err(e) => {
            println!("bsdnet: if_em did not attach (FreeBSD errno {e})");
            return 1;
        }
    };
    // Interrupts and the kernel's frames come to the main port.
    let msi = {
        let host = ffi::HOST.lock();
        let Some(h) = host.as_ref() else {
            return 1;
        };
        handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND | Rights::TRANSFER)
            .ok()
            .and_then(|p| h.device.irq(p).ok())
    };
    let Ok(port) = handle_duplicate(bootstrap::MAIN_PORT, Rights::SEND | Rights::TRANSFER) else {
        return 1;
    };
    match NetTap::attach(port, mac) {
        Ok(t) => {
            if let Some(h) = ffi::HOST.lock().as_mut() {
                h.tap = Some(t);
            }
        }
        Err(e) => {
            println!("bsdnet: cannot attach to the network: {}", e.name());
            return 1;
        }
    }
    println!(
        "bsdnet: em0 MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, {}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        match msi {
            Some(v) => format!("MSI vector {v}"),
            None => alloc::string::String::from("polled"),
        }
    );
    let mut frame = [0u8; 2048];
    let mut msg = Message::new();
    let mut irqs = 0u64;
    loop {
        let timeout = if msi.is_some() { 20_000_000 } else { 5_000_000 };
        if port_receive(bootstrap::MAIN_PORT, &mut msg, timeout).is_ok() {
            if msg.tag == tag::DEVICE_IRQ {
                irqs += 1;
                if irqs == 1 {
                    println!("bsdnet: first interrupt");
                }
            } else if msg.tag != tag::NET_TX_READY {
                for &h in msg.carried() {
                    let _ = libstela::ipc::handle_close(h);
                }
            }
        }
        ffi::interrupt();
        ffi::tick();
        loop {
            let n = {
                let host = ffi::HOST.lock();
                match host.as_ref().and_then(|h| h.tap.as_ref()) {
                    Some(tap) => tap.take(&mut frame).unwrap_or(0),
                    None => 0,
                }
            };
            if n == 0 || !ffi::transmit(&frame[..n]) {
                break;
            }
        }
    }
}
