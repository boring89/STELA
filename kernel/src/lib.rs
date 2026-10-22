// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA kernel's logic: threads, processes, scheduling and system
//! calls. Hardware access goes
//! through `frame`; the parts that are pure logic also build on the host
//! for `cargo test`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(target_os = "none")]
pub mod audio;
pub mod audiofeed;
#[cfg(target_os = "none")]
pub mod block;
#[cfg(target_os = "none")]
pub mod console;
#[cfg(target_os = "none")]
pub mod device;
pub mod fs;
#[cfg(target_os = "none")]
pub mod fsyscall;
pub mod hwinfo;
pub mod initrd;
#[cfg(target_os = "none")]
pub mod input;
pub mod linux;
#[cfg(target_os = "none")]
pub mod net;
pub mod object;
#[cfg(target_os = "none")]
pub mod power;
#[cfg(target_os = "none")]
pub mod program;
#[cfg(target_os = "none")]
pub mod psyscall;
#[cfg(target_os = "none")]
pub mod syscall;
pub mod task;
#[cfg(target_os = "none")]
pub mod usb;
pub mod vfs;
