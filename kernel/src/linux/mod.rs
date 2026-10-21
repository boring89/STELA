// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Linux personality (`docs/linux.md`): static x86-64 Linux programs
//! (musl) run on STELA. They enter the kernel through the same `syscall`
//! instruction; `syscall::handle` sends their calls here, where the Linux
//! numbers (`nr`) are translated to STELA's objects: descriptors (`fd`)
//! name ttys and files, paths (`path`) are mapped onto the volumes, and
//! the result goes back in RAX alone (negative = `-errno`).
//!
//! The pure parts (numbers, errno, descriptor table, paths, the initial
//! stack) build on the host for `cargo test`.

pub mod errno;
pub mod fd;
pub mod nr;
pub mod path;
pub mod signal;
pub mod stack;

#[cfg(target_os = "none")]
mod files;
#[cfg(target_os = "none")]
mod futex;
#[cfg(target_os = "none")]
mod memory;
#[cfg(target_os = "none")]
mod poll;
#[cfg(target_os = "none")]
mod process;
#[cfg(target_os = "none")]
pub mod signals;
#[cfg(target_os = "none")]
mod socket;
#[cfg(target_os = "none")]
mod state;
#[cfg(target_os = "none")]
mod sys;

#[cfg(target_os = "none")]
pub use process::process_ended;
#[cfg(target_os = "none")]
pub use signals::{deliver, fault_signal};
#[cfg(target_os = "none")]
pub use state::{FileState, LinuxThread, OpenFile, State};
#[cfg(target_os = "none")]
pub use sys::{dispatch, initial_stack};
