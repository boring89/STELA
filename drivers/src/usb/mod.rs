// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! USB (`docs/usb.md`): the xHCI host controller ([`xhci`]), hubs and HID
//! boot keyboards and mice. This module holds the pure logic, tested on
//! the host: standard requests and descriptors (USB 2.0 Specification
//! chapter 9), hub requests (chapter 11), and the HID boot reports (Device
//! Class Definition for HID 1.11, appendix B; HID Usage Tables 1.12,
//! chapter 10 "Keyboard/Keypad Page").

pub mod hid;
pub mod trb;
#[cfg(target_os = "none")]
pub mod xhci;

use alloc::vec::Vec;

/// A SETUP packet (USB 2.0 9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    /// Direction, type, recipient.
    pub request_type: u8,
    /// The request.
    pub request: u8,
    /// Request-specific.
    pub value: u16,
    /// Request-specific (an interface or port number).
    pub index: u16,
    /// Bytes in the data stage.
    pub length: u16,
}

/// `bmRequestType` bit 7: device to host.
pub const DIR_IN: u8 = 0x80;

/// Standard request codes (USB 2.0 Table 9-4).
pub mod request {
    #![allow(missing_docs)]
    pub const GET_STATUS: u8 = 0;
    pub const CLEAR_FEATURE: u8 = 1;
    pub const SET_FEATURE: u8 = 3;
    pub const GET_DESCRIPTOR: u8 = 6;
    pub const SET_CONFIGURATION: u8 = 9;
    /// HID class requests (HID 1.11 7.2).
    pub const HID_SET_IDLE: u8 = 0x0A;
    pub const HID_SET_PROTOCOL: u8 = 0x0B;
}

/// Descriptor types (USB 2.0 Table 9-5; hub: 11.23.2.1).
pub mod descriptor {
    #![allow(missing_docs)]
    pub const DEVICE: u8 = 1;
    pub const CONFIGURATION: u8 = 2;
    pub const INTERFACE: u8 = 4;
    pub const ENDPOINT: u8 = 5;
    pub const HUB: u8 = 0x29;
}

/// Hub port features (USB 2.0 Table 11-17).
pub mod port_feature {
    #![allow(missing_docs)]
    pub const RESET: u16 = 4;
    pub const POWER: u16 = 8;
    pub const C_CONNECTION: u16 = 16;
    pub const C_ENABLE: u16 = 17;
    pub const C_RESET: u16 = 20;
}

impl Setup {
    /// The 8 bytes as sent.
    pub fn to_bytes(&self) -> [u8; 8] {
        let v = self.value.to_le_bytes();
        let i = self.index.to_le_bytes();
        let l = self.length.to_le_bytes();
        [
            self.request_type,
            self.request,
            v[0],
            v[1],
            i[0],
            i[1],
            l[0],
            l[1],
        ]
    }

    /// The packet as one little-endian u64 (an xHCI Setup TRB's
    /// parameter, "immediate data").
    pub fn to_u64(&self) -> u64 {
        u64::from_le_bytes(self.to_bytes())
    }

    /// GET_DESCRIPTOR of `kind`, index `index`, `length` bytes.
    pub const fn get_descriptor(kind: u8, index: u8, length: u16) -> Setup {
        Setup {
            request_type: DIR_IN,
            request: request::GET_DESCRIPTOR,
            value: (kind as u16) << 8 | index as u16,
            index: 0,
            length,
        }
    }

    /// SET_CONFIGURATION.
    pub const fn set_configuration(value: u8) -> Setup {
        Setup {
            request_type: 0,
            request: request::SET_CONFIGURATION,
            value: value as u16,
            index: 0,
            length: 0,
        }
    }

    /// HID SET_PROTOCOL (0 = boot) to `interface`.
    pub const fn set_protocol(interface: u8, protocol: u16) -> Setup {
        Setup {
            request_type: 0x21,
            request: request::HID_SET_PROTOCOL,
            value: protocol,
            index: interface as u16,
            length: 0,
        }
    }

    /// HID SET_IDLE (0 = report only on change) to `interface`.
    pub const fn set_idle(interface: u8) -> Setup {
        Setup {
            request_type: 0x21,
            request: request::HID_SET_IDLE,
            value: 0,
            index: interface as u16,
            length: 0,
        }
    }

    /// The hub descriptor (class request to the device).
    pub const fn get_hub_descriptor(length: u16) -> Setup {
        Setup {
            request_type: DIR_IN | 0x20,
            request: request::GET_DESCRIPTOR,
            value: (descriptor::HUB as u16) << 8,
            index: 0,
            length,
        }
    }

    /// GET_STATUS of hub port `port` (4 bytes: status, change).
    pub const fn get_port_status(port: u8) -> Setup {
        Setup {
            request_type: DIR_IN | 0x23,
            request: request::GET_STATUS,
            value: 0,
            index: port as u16,
            length: 4,
        }
    }

    /// SET_FEATURE (`set`) or CLEAR_FEATURE of hub port `port`.
    pub const fn port_feature(port: u8, feature: u16, set: bool) -> Setup {
        Setup {
            request_type: 0x23,
            request: if set {
                request::SET_FEATURE
            } else {
                request::CLEAR_FEATURE
            },
            value: feature,
            index: port as u16,
            length: 0,
        }
    }
}

/// What a device descriptor says (USB 2.0 Table 9-8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceDescriptor {
    /// USB version (BCD).
    pub usb: u16,
    /// Device class (0: per interface; 9: hub).
    pub class: u8,
    /// Endpoint 0's packet size (byte 7).
    pub max_packet0: u8,
    /// Vendor id.
    pub vendor: u16,
    /// Product id.
    pub product: u16,
    /// Number of configurations.
    pub configurations: u8,
}

/// Parses a device descriptor (at least 18 bytes).
pub fn parse_device(b: &[u8]) -> Option<DeviceDescriptor> {
    if b.len() < 18 || b[1] != descriptor::DEVICE {
        return None;
    }
    Some(DeviceDescriptor {
        usb: u16::from_le_bytes([b[2], b[3]]),
        class: b[4],
        max_packet0: b[7],
        vendor: u16::from_le_bytes([b[8], b[9]]),
        product: u16::from_le_bytes([b[10], b[11]]),
        configurations: b[17],
    })
}

/// One endpoint of an interface (USB 2.0 Table 9-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// Address: number in bits 3:0, bit 7 IN.
    pub address: u8,
    /// Transfer type: 0 control, 1 isochronous, 2 bulk, 3 interrupt.
    pub kind: u8,
    /// Largest packet.
    pub max_packet: u16,
    /// Polling interval (frames or 2^(n-1) microframes).
    pub interval: u8,
}

impl Endpoint {
    /// An IN endpoint.
    pub const fn is_in(&self) -> bool {
        self.address & 0x80 != 0
    }

    /// The xHCI device context index: 2 * number + IN (xHCI 1.2 4.5.1).
    pub const fn dci(&self) -> u8 {
        (self.address & 0x0F) * 2 + if self.is_in() { 1 } else { 0 }
    }
}

/// One interface (USB 2.0 Table 9-12) and its endpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    /// Its number.
    pub number: u8,
    /// Class (3 HID, 9 hub).
    pub class: u8,
    /// Subclass (HID: 1 boot).
    pub subclass: u8,
    /// Protocol (HID boot: 1 keyboard, 2 mouse).
    pub protocol: u8,
    /// Its endpoints.
    pub endpoints: Vec<Endpoint>,
}

/// A configuration (USB 2.0 Table 9-10): its value and interfaces
/// (alternate settings other than 0 are skipped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    /// `bConfigurationValue` (for SET_CONFIGURATION).
    pub value: u8,
    /// The interfaces.
    pub interfaces: Vec<Interface>,
}

/// The total length of a configuration from its first 9 bytes.
pub fn configuration_length(b: &[u8]) -> Option<u16> {
    (b.len() >= 9 && b[1] == descriptor::CONFIGURATION).then(|| u16::from_le_bytes([b[2], b[3]]))
}

/// Parses a whole configuration descriptor set.
pub fn parse_configuration(b: &[u8]) -> Option<Configuration> {
    if b.len() < 9 || b[1] != descriptor::CONFIGURATION {
        return None;
    }
    let mut config = Configuration {
        value: b[5],
        interfaces: Vec::new(),
    };
    let mut at = 0;
    let mut skipping = false;
    while at + 2 <= b.len() {
        let len = b[at] as usize;
        if len < 2 || at + len > b.len() {
            break;
        }
        let d = &b[at..at + len];
        match d[1] {
            descriptor::INTERFACE if len >= 9 => {
                // Only alternate setting 0.
                skipping = d[3] != 0;
                if !skipping {
                    config.interfaces.push(Interface {
                        number: d[2],
                        class: d[5],
                        subclass: d[6],
                        protocol: d[7],
                        endpoints: Vec::new(),
                    });
                }
            }
            descriptor::ENDPOINT if len >= 7 && !skipping => {
                if let Some(i) = config.interfaces.last_mut() {
                    i.endpoints.push(Endpoint {
                        address: d[2],
                        kind: d[3] & 3,
                        max_packet: u16::from_le_bytes([d[4], d[5]]) & 0x7FF,
                        interval: d[6],
                    });
                }
            }
            _ => {}
        }
        at += len;
    }
    Some(config)
}

/// Device speeds, as xHCI numbers them (PORTSC bits 13:10, the default
/// Protocol Speed IDs: xHCI 1.2 7.2.2.1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    /// 12 Mb/s.
    Full = 1,
    /// 1.5 Mb/s.
    Low = 2,
    /// 480 Mb/s.
    High = 3,
    /// 5 Gb/s.
    Super = 4,
}

impl Speed {
    /// From the PORTSC speed field.
    pub fn from_raw(v: u32) -> Option<Speed> {
        match v {
            1 => Some(Speed::Full),
            2 => Some(Speed::Low),
            3 => Some(Speed::High),
            4 => Some(Speed::Super),
            _ => None,
        }
    }

    /// Endpoint 0's packet size before the device descriptor is read.
    pub const fn default_max_packet0(self) -> u16 {
        match self {
            Speed::Low | Speed::Full => 8,
            Speed::High => 64,
            Speed::Super => 512,
        }
    }

    /// A short name.
    pub const fn name(self) -> &'static str {
        match self {
            Speed::Low => "low",
            Speed::Full => "full",
            Speed::High => "high",
            Speed::Super => "super",
        }
    }
}

/// The xHCI endpoint context Interval for an interrupt endpoint with
/// `bInterval` (xHCI 1.2 6.2.3.6): high/super speed already use 2^(n-1)
/// microframes; low/full speed give frames (1 ms = 8 microframes), so the
/// interval is log2(8 * frames), between 3 and 10.
pub fn interrupt_interval(speed: Speed, b_interval: u8) -> u8 {
    match speed {
        Speed::High | Speed::Super => b_interval.clamp(1, 16) - 1,
        Speed::Low | Speed::Full => {
            let micro = u32::from(b_interval.max(1)) * 8;
            (31 - micro.leading_zeros()).clamp(3, 10) as u8
        }
    }
}

/// A USB 2.0 hub's port status (Table 11-21) and change bits (Table
/// 11-22).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortStatus {
    /// Something is connected.
    pub connected: bool,
    /// The port is enabled.
    pub enabled: bool,
    /// A reset is going on.
    pub resetting: bool,
    /// The device's speed.
    pub speed: Speed,
    /// The connection changed.
    pub connect_changed: bool,
    /// A reset finished.
    pub reset_changed: bool,
}

/// Parses the 4 bytes of a hub port GET_STATUS.
pub fn parse_port_status(b: &[u8]) -> Option<PortStatus> {
    let b = b.get(..4)?;
    let status = u16::from_le_bytes([b[0], b[1]]);
    let change = u16::from_le_bytes([b[2], b[3]]);
    let speed = if status & (1 << 9) != 0 {
        Speed::Low
    } else if status & (1 << 10) != 0 {
        Speed::High
    } else {
        Speed::Full
    };
    Some(PortStatus {
        connected: status & 1 != 0,
        enabled: status & 2 != 0,
        resetting: status & (1 << 4) != 0,
        speed,
        connect_changed: change & 1 != 0,
        reset_changed: change & (1 << 4) != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn setup_packets() {
        let s = Setup::get_descriptor(descriptor::DEVICE, 0, 18);
        assert_eq!(s.to_bytes(), [0x80, 6, 0, 1, 0, 0, 18, 0]);
        assert_eq!(s.to_u64(), 0x0012_0000_0100_0680);
        assert_eq!(
            Setup::set_configuration(1).to_bytes(),
            [0, 9, 1, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            Setup::set_protocol(2, 0).to_bytes(),
            [0x21, 0x0B, 0, 0, 2, 0, 0, 0]
        );
        assert_eq!(
            Setup::get_hub_descriptor(9).to_bytes(),
            [0xA0, 6, 0, 0x29, 0, 0, 9, 0]
        );
        assert_eq!(
            Setup::get_port_status(3).to_bytes(),
            [0xA3, 0, 0, 0, 3, 0, 4, 0]
        );
        assert_eq!(
            Setup::port_feature(2, port_feature::RESET, true).to_bytes(),
            [0x23, 3, 4, 0, 2, 0, 0, 0]
        );
        assert_eq!(
            Setup::port_feature(2, port_feature::C_RESET, false).request,
            1
        );
    }

    #[test]
    fn device_descriptor() {
        // QEMU's usb-kbd.
        let d = [
            18, 1, 0x00, 0x02, 0, 0, 0, 8, 0x27, 0x06, 0x01, 0x00, 0, 0, 1, 2, 3, 1,
        ];
        let dev = parse_device(&d).unwrap();
        assert_eq!(
            (dev.usb, dev.max_packet0, dev.vendor, dev.product),
            (0x200, 8, 0x0627, 1)
        );
        assert_eq!(parse_device(&d[..10]), None);
    }

    #[test]
    fn configuration() {
        #[rustfmt::skip]
        let c = vec![
            9, 2, 34, 0, 1, 1, 0, 0xA0, 50,         // configuration, 34 bytes
            9, 4, 0, 0, 1, 3, 1, 1, 0,              // interface 0: HID boot keyboard
            9, 0x21, 0x11, 1, 0, 1, 0x22, 63, 0,    // HID descriptor (skipped)
            7, 5, 0x81, 3, 8, 0, 10,                // endpoint 1 IN, interrupt, 8 bytes
        ];
        assert_eq!(configuration_length(&c), Some(34));
        let config = parse_configuration(&c).unwrap();
        assert_eq!(config.value, 1);
        assert_eq!(config.interfaces.len(), 1);
        let i = &config.interfaces[0];
        assert_eq!((i.class, i.subclass, i.protocol), (3, 1, 1));
        let e = i.endpoints[0];
        assert_eq!(
            (e.address, e.kind, e.max_packet, e.interval),
            (0x81, 3, 8, 10)
        );
        assert!(e.is_in());
        assert_eq!(e.dci(), 3);
        // A zero-length descriptor stops the walk instead of looping.
        let mut bad = c.clone();
        bad[9] = 0;
        assert_eq!(parse_configuration(&bad).unwrap().interfaces.len(), 0);
    }

    #[test]
    fn intervals_and_speeds() {
        assert_eq!(interrupt_interval(Speed::Full, 10), 6); // 80 microframes -> 2^6
        assert_eq!(interrupt_interval(Speed::Low, 1), 3);
        assert_eq!(interrupt_interval(Speed::Full, 255), 10);
        assert_eq!(interrupt_interval(Speed::High, 4), 3);
        assert_eq!(Speed::from_raw(3), Some(Speed::High));
        assert_eq!(Speed::Super.default_max_packet0(), 512);
    }

    #[test]
    fn hub_port_status() {
        let s = parse_port_status(&[0x03, 0x03, 0x11, 0x00]).unwrap();
        assert!(s.connected && s.enabled && s.connect_changed && s.reset_changed);
        assert_eq!(s.speed, Speed::Low);
        let s = parse_port_status(&[0x01, 0x04, 0, 0]).unwrap();
        assert_eq!(s.speed, Speed::High);
        assert!(!s.enabled);
    }
}
