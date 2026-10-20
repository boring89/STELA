// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Kernel objects and handles (`docs/ipc.md`).
//!
//! A process names kernel objects through handles: indices into its own
//! handle table, each carrying a set of rights. The objects are reference
//! counted (`Arc`); an object lives while any handle, queued message or
//! kernel structure refers to it.

pub mod handle;
pub mod ldisc;
#[cfg(target_os = "none")]
pub mod port;
pub mod regions;
#[cfg(target_os = "none")]
pub mod tty;

#[cfg(target_os = "none")]
pub use target::*;

#[cfg(target_os = "none")]
mod target {
    use super::port::Port;
    use super::tty::Tty;
    use crate::vfs::Node;
    use alloc::sync::Arc;
    use frame::mm::shared::SharedMemory;

    /// Something a handle can refer to.
    #[derive(Clone)]
    pub enum KernelObject {
        /// A message queue.
        Port(Arc<Port>),
        /// Shared memory (the frames are reference counted inside).
        Memory(SharedMemory),
        /// A folder on a volume.
        Directory(Node),
        /// A file on a volume.
        File(Node),
        /// A process, by id (the handle does not keep its memory alive;
        /// its exit status stays in the process table).
        Process(crate::task::ProcessId),
        /// The program side of a terminal: `PortSend` of `CONSOLE_WRITE`
        /// prints, of `INPUT_KEY` types (`docs/linux.md` §5).
        Tty(Arc<Tty>),
        /// The master side of a terminal (`TtyRead`, `TtyWrite`).
        TtyMaster(Arc<Tty>),
        /// The boot framebuffer (`MemoryMap` maps it write-combining;
        /// `docs/display.md`).
        Framebuffer(Arc<frame::console::FramebufferDevice>),
        /// A network socket (`docs/net.md`).
        Socket(Arc<crate::net::Socket>),
        /// The open sound output (`docs/sound.md`).
        Audio(Arc<crate::audio::AudioSession>),
    }

    impl KernelObject {
        /// The port, if this is one.
        pub fn as_port(&self) -> Option<&Arc<Port>> {
            match self {
                KernelObject::Port(port) => Some(port),
                _ => None,
            }
        }

        /// The folder, if this is one.
        pub fn as_directory(&self) -> Option<&Node> {
            match self {
                KernelObject::Directory(node) => Some(node),
                _ => None,
            }
        }

        /// The file, if this is one.
        pub fn as_file(&self) -> Option<&Node> {
            match self {
                KernelObject::File(node) => Some(node),
                _ => None,
            }
        }

        /// The terminal, if this is its program side.
        pub fn as_tty(&self) -> Option<&Arc<Tty>> {
            match self {
                KernelObject::Tty(tty) => Some(tty),
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
