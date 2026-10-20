// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Kernel objects and handles (`docs/ipc.md`).
//!
//! A process names kernel objects through handles: indices into its own
//! handle table, each carrying a set of rights. The objects are reference
//! counted (`Arc`); an object lives while any handle, queued message or
//! kernel structure refers to it.

pub mod handle;
#[cfg(target_os = "none")]
pub mod port;
pub mod regions;

#[cfg(target_os = "none")]
pub use target::*;

#[cfg(target_os = "none")]
mod target {
    use super::port::Port;
    use alloc::sync::Arc;
    use frame::mm::shared::SharedMemory;

    /// Something a handle can refer to.
    #[derive(Clone)]
    pub enum KernelObject {
        /// A message queue.
        Port(Arc<Port>),
        /// Shared memory (the frames are reference counted inside).
        Memory(SharedMemory),
    }

    impl KernelObject {
        /// The port, if this is one.
        pub fn as_port(&self) -> Option<&Arc<Port>> {
            match self {
                KernelObject::Port(port) => Some(port),
                _ => None,
            }
        }

        /// The memory, if this is shared memory.
        pub fn as_memory(&self) -> Option<&SharedMemory> {
            match self {
                KernelObject::Memory(memory) => Some(memory),
                _ => None,
            }
        }
    }

    /// A handle-table entry of a process.
    pub type Entry = super::handle::Entry<KernelObject>;
    /// A process's handle table.
    pub type HandleTable = super::handle::HandleTable<KernelObject>;
}
