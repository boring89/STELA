// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA UI toolbox (`docs/toolbox.md`). Its modules follow the
//! Classic Mac OS managers:
//!
//! - [`draw`] (QuickDraw): rectangles, lines, bitmaps and text on a
//!   pixel buffer, clipped.
//! - [`window`] (Window Manager): the connection to the window server,
//!   windows and their pixels.
//! - [`event`] (Event Manager): decoded events.
//! - [`menu`] (Menu Manager): menu bars.
//! - [`control`] (Control Manager): buttons, text fields, scroll bars,
//!   lists.
//!
//! The drawing, event, menu and control code is pure logic and is tested
//! on the host; [`window`] makes the system calls.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod control;
pub mod draw;
pub mod event;
pub mod menu;
#[cfg(not(test))]
pub mod window;
