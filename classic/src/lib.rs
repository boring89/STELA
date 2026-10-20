// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Classic environment (`docs/classic.md`): a Macintosh IIci in a
//! window. The 68k core is Musashi and the machine model is 68k_HAL's
//! `emu/mac`, both C (`classic/c`, `classic/musashi`, built by `build.rs`);
//! this library wraps them in a safe interface ([`ffi`], the only module
//! with `unsafe` code) and holds the pure logic the program (`main.rs`)
//! uses, so that `cargo test --lib` runs it on the host.

#![cfg_attr(not(test), no_std)]
#![deny(unsafe_code)]

extern crate alloc;

pub mod ffi;
pub mod keys;
pub mod screen;
pub mod sound;

#[cfg(test)]
mod tests;
