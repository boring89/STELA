// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! File systems and the block cache (`docs/storage.md`, `docs/vfs.md`).

pub mod cache;
pub mod fat;

/// The time stamps file systems write: nanoseconds since 1970. There is
/// no real-time clock driver yet, so this is the time since boot counted
/// from 2026-01-01 00:00 UTC (a fixed, recognisable epoch).
#[cfg(target_os = "none")]
pub fn now() -> u64 {
    const EPOCH_2026: u64 = 1_767_225_600 * 1_000_000_000;
    EPOCH_2026 + frame::time::now_ns()
}
