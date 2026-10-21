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
//! - [`text`] (TextEdit, P11): multi-line text with selection, wrapping
//!   and undo, and a scrolled view of it.
//! - [`dialog`] (Dialog Manager, P11): alerts, a text question, Open
//!   and Save dialogs.
//! - [`icons`]: STELA's icons (drafts) from `assets/`.
//!
//! The drawing, event, menu and control code is pure logic and is tested
//! on the host; [`window`] makes the system calls.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod control;
pub mod dialog;
pub mod draw;
pub mod event;
pub mod icons;
pub mod menu;
pub mod text;
#[cfg(not(test))]
pub mod window;
