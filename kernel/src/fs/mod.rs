// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! File systems and the block cache (`docs/storage.md`, `docs/vfs.md`).

pub mod cache;
pub mod fat;

/// The time stamps file systems write: nanoseconds since 1970 (UTC), from
/// the wall clock (the RTC read at boot; 2026-01-01 if it could not be
/// read).
#[cfg(target_os = "none")]
pub fn now() -> u64 {
    frame::time::wall_clock_ns()
}
