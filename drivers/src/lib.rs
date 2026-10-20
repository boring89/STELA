// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! STELA's device drivers, in safe Rust: they reach the hardware only
//! through `frame` (MMIO regions, configuration ports, DMA buffers,
//! interrupt vectors). The parts that are pure logic (PCI enumeration over
//! a configuration-space trait, register layouts, command encodings) build
//! on the host and have unit tests.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod ahci;
pub mod hda;
pub mod net;
pub mod nvme;
pub mod pci;
pub mod sync;
pub mod usb;
