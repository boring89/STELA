// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! STELA's network protocols (`docs/net.md`), as pure logic: packet
//! formats ([`wire`]), the DHCP client's messages ([`dhcp`]), TCP
//! connections ([`tcp`]) and the stack of one interface ([`stack`]) that
//! ties them together. Nothing here touches a device or a clock: frames
//! and the time are handed in, frames to send are taken out, so that the
//! whole stack runs in host tests.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod dhcp;
pub mod stack;
pub mod tcp;
pub mod wire;

pub use stack::{Config, Kind, NetError, SocketId, Stack};
pub use wire::{Ip, Mac};

#[cfg(test)]
mod tests;
