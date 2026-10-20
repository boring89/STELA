// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! windowserver: owns the screen, the keyboard and the mouse, and shows
//! the windows of its clients (`docs/window.md`).
//!
//! ```text
//!  [2] Framebuffer ──MemoryMap──► the screen (write-combining)
//!  back buffer (own memory, same size): the scene is composited here,
//!      dirty rectangles only (desktop → windows back to front → frames →
//!      menu bar and menus → drag outline), then copied to the screen with
//!      the pointer drawn over it
//!  main port ◄── INPUT_KEY / INPUT_MOUSE (InputAttach) and requests
//!                (abi::window); clients get a SEND handle as their [2]
//! ```
//!
//! Arguments: programs to start from `Initrd:/` once the screen is up
//! (`windowserver terminal`).

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

mod server;

libstela::entry!(server::main);
