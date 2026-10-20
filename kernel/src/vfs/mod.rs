// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The virtual file system (`docs/vfs.md`): volumes, each its own root
//! (`Volume:/dir/file`), and handles to folders and files on them.
//!
//! ```text
//!  VOLUMES ── Arc<Volume> "Data"  (eHFS, rw) ──► FileSystem backend
//!          ├─ Arc<Volume> "Boot"  (FAT, ro)
//!          └─ ...
//!  handle ──► KernelObject::Directory(Node) / File(Node)
//!             Node = (Arc<Volume>, persistent id)
//! ```
//!
//! Every operation is relative to a folder handle; nothing is reached by
//! a global path except `VolumeOpen`. A node is named by its volume's
//! persistent id (eHFS: the CNID), so a handle stays valid when the item
//! is renamed or moved.

#[cfg(target_os = "none")]
pub mod mount;
pub mod path;

#[cfg(target_os = "none")]
mod target;
#[cfg(target_os = "none")]
pub use target::*;
