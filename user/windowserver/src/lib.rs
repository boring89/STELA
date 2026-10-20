// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window server's pure logic (`compose`), built as a library so that
//! `cargo test --lib` runs it on the host; the program is `main.rs`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod compose;
