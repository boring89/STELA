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
pub mod pipe;
#[cfg(target_os = "none")]
pub mod port;
pub mod regions;
#[cfg(target_os = "none")]
pub mod thread;
#[cfg(target_os = "none")]
pub mod tty;
#[cfg(target_os = "none")]
pub mod waitset;

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
        /// Objects to wait on together (`docs/ipc.md` §13).
        WaitSet(Arc<super::waitset::WaitSet>),
        /// A capability: holding it is the permission (`abi::capability`;
        /// `docs/userland.md` §2.3).
        Capability(u32),
        /// A pipe's reading end (`docs/ipc.md` §14).
        PipeRead(Arc<super::pipe::ReadEnd>),
        /// A pipe's writing end.
        PipeWrite(Arc<super::pipe::WriteEnd>),
        /// A thread's end (`ThreadJoin`; `docs/process.md` §8.1).
        Thread(Arc<super::thread::ThreadExit>),
        /// A PCI function lent to a user-space driver (P13 K11).
        Device(Arc<crate::device::Device>),
        /// A user-space network card attached to the kernel (P13 K11).
        NetTap(Arc<crate::device::TapHandle>),
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

    /// A capability handle of `kind` (`DUPLICATE | TRANSFER`).
    pub fn capability(kind: u32) -> Entry {
        Entry {
            object: KernelObject::Capability(kind),
            rights: abi::Rights::DUPLICATE | abi::Rights::TRANSFER,
        }
    }

    /// Whether `process` holds a capability of `kind` in handle `handle`.
    pub fn check_capability(
        process: &crate::task::Process,
        handle: u64,
        kind: u32,
    ) -> Result<(), abi::Error> {
        let handles = process.handles().lock();
        match handles.get(abi::Handle::from_raw(handle)) {
            Ok(Entry {
                object: KernelObject::Capability(k),
                ..
            }) if *k == kind => Ok(()),
            Ok(_) => Err(abi::Error::AccessDenied),
            Err(abi::Error::BadHandle) => Err(abi::Error::AccessDenied),
            Err(e) => Err(e),
        }
    }

    /// A handle-table entry of a process.
    pub type Entry = super::handle::Entry<KernelObject>;
    /// A process's handle table.
    pub type HandleTable = super::handle::HandleTable<KernelObject>;
}
