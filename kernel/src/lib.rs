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
pub mod syscall;
pub mod task;
