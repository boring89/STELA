// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Threads and the scheduler.
//!
//! The policy lives here, in safe Rust: which thread runs next, time
//! slices, sleeping and blocking. The mechanism (saving one context and
//! continuing another, kernel stacks) is `frame::task`.

#[cfg(target_os = "none")]
pub mod process;
pub mod queue;
#[cfg(target_os = "none")]
mod sched;

#[cfg(target_os = "none")]
pub use process::{Personality, Process, ProcessId, wait_process};
pub use queue::WaitQueue;
#[cfg(target_os = "none")]
pub use sched::*;

/// A thread's id: global, increasing, never reused.
pub type ThreadId = u64;

/// Time slice in timer ticks (10 ms at 1000 Hz).
pub const TIME_SLICE: u32 = 10;
