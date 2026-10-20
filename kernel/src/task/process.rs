// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Processes: an address space and the threads that run in it.
//!
//! A process ends when its last thread has been freed (reaped). Its exit
//! code is the one given to the first `ProcessExit` (or the kill, for a
//! fault), or 0 when the threads simply exited. The status stays in the
//! process table for [`wait_process`]; the address space is freed with the
//! last reference to the [`Process`], which only its threads hold. A
//! process dropped without ever running a thread ends with code -1.

use super::ThreadId;
use crate::object::port::Port;
use crate::object::regions::Regions;
use crate::object::{Entry, HandleTable, KernelObject};
use abi::Rights;
use abi::layout::{MAP_BASE, MAP_END};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use frame::mm::paging::MapError;
use frame::mm::user::AddressSpace;
use frame::sync::SpinLock;

/// A process's id: global, increasing, never reused.
pub type ProcessId = u64;

/// A process: what its threads share.
pub struct Process {
    id: ProcessId,
    name: String,
    space: AddressSpace,
    /// Threads created and not yet reaped.
    live: AtomicUsize,
    /// The exit code once one is decided (first exit or kill wins).
    exit_code: SpinLock<Option<i64>>,
    /// The process's handles.
    handles: SpinLock<HandleTable>,
    /// Its main port (also handle `[0]`).
    main_port: Arc<Port>,
    /// Ports it created: closed when it ends.
    ports: SpinLock<Vec<Weak<Port>>>,
    /// The `MemoryMap` ranges in its map area.
    maps: SpinLock<Regions>,
}

impl Process {
    /// The process id.
    pub fn id(&self) -> ProcessId {
        self.id
    }

    /// The name it was started with.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its address space.
    pub fn space(&self) -> &AddressSpace {
        &self.space
    }

    /// Its handle table.
    pub fn handles(&self) -> &SpinLock<HandleTable> {
        &self.handles
    }

    /// The ranges of its map area in use.
    pub fn maps(&self) -> &SpinLock<Regions> {
        &self.maps
    }

    /// Its main port.
    pub fn main_port(&self) -> &Arc<Port> {
        &self.main_port
    }

    /// A new port owned by this process, closed when it ends.
    pub fn new_port(&self) -> Arc<Port> {
        let port = Port::new(self.id);
        let mut ports = self.ports.lock();
        ports.retain(|p| p.strong_count() > 0);
        ports.push(Arc::downgrade(&port));
        port
    }

    /// Releases what the process holds when it ends: its handles (dropped
    /// after the lock is released) and its ports (closed).
    fn release(&self) {
        let entries = self.handles.lock().drain();
        drop(entries);
        self.main_port.close();
        let ports = core::mem::take(&mut *self.ports.lock());
        for port in ports.iter().filter_map(Weak::upgrade) {
            port.close();
        }
    }

    /// Records `code` unless an exit code was already decided. Returns
    /// whether this call decided it.
    pub(crate) fn set_exit_code(&self, code: i64) -> bool {
        let mut exit = self.exit_code.lock();
        if exit.is_some() {
            return false;
        }
        *exit = Some(code);
        true
    }

    /// Whether the process is being torn down.
    pub fn exiting(&self) -> bool {
        self.exit_code.lock().is_some()
    }

    pub(crate) fn thread_added(&self) {
        self.live.fetch_add(1, Ordering::Relaxed);
    }

    /// Called when one of its threads has been freed; the last one ends
    /// the process.
    pub(crate) fn thread_reaped(&self) {
        if self.live.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.release();
            let code = self.exit_code.lock().unwrap_or(0);
            finish(self.id, code);
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // A process that never got a thread (its program failed to load)
        // ends here, so that waiting for it does not hang.
        let never_ran = matches!(TABLE.lock().get(&self.id), Some(Status::Running { .. }));
        if never_ran {
            self.release();
            finish(self.id, -1);
        }
    }
}

/// What the process table remembers.
enum Status {
    Running { waiters: Vec<ThreadId> },
    Exited(i64),
}

static TABLE: SpinLock<BTreeMap<ProcessId, Status>> = SpinLock::new(BTreeMap::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Creates a process with an empty address space, no threads and the
/// bootstrap handles: `[0]` its main port, `[1]` the console service (if
/// it runs), `[2]` nothing, `[3]` the root of `Data` (if mounted).
pub fn create(name: &str) -> Result<Arc<Process>, MapError> {
    create_with(name, None)
}

/// Like [`create`], with `bootstrap` as handle `[2]`.
pub fn create_with(name: &str, bootstrap: Option<Entry>) -> Result<Arc<Process>, MapError> {
    let space = AddressSpace::new()?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let main_port = Port::new(id);
    let main = Entry {
        object: KernelObject::Port(Arc::clone(&main_port)),
        rights: Rights::RECEIVE | Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
    };
    let console = crate::console::service_port().map(|port| Entry {
        object: KernelObject::Port(port),
        rights: Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
    });
    let cwd = crate::vfs::find("Data").map(|volume| {
        let rights = crate::fsyscall::root_rights(&volume);
        let id = volume.fs.root();
        Entry {
            object: KernelObject::Directory(crate::vfs::Node { volume, id }),
            rights,
        }
    });
    let handles = HandleTable::with_initial([Some(main), console, bootstrap, cwd]);
    TABLE.lock().insert(
        id,
        Status::Running {
            waiters: Vec::new(),
        },
    );
    Ok(Arc::new(Process {
        id,
        name: String::from(name),
        space,
        live: AtomicUsize::new(0),
        exit_code: SpinLock::new(None),
        handles: SpinLock::new(handles),
        main_port,
        ports: SpinLock::new(Vec::new()),
        maps: SpinLock::new(Regions::new(MAP_BASE, MAP_END)),
    }))
}

/// Records the end of process `id` and wakes its waiters.
fn finish(id: ProcessId, code: i64) {
    let waiters = {
        let mut table = TABLE.lock();
        match table.insert(id, Status::Exited(code)) {
            Some(Status::Running { waiters }) => waiters,
            _ => Vec::new(),
        }
    };
    for waiter in waiters {
        super::wake(waiter);
    }
}

/// Waits until process `id` has ended and returns its exit code (`None`
/// for an unknown id). The status stays, so waiting again returns it again.
pub fn wait_process(id: ProcessId) -> Option<i64> {
    let me = super::current_id();
    loop {
        {
            let mut table = TABLE.lock();
            match table.get_mut(&id)? {
                Status::Exited(code) => return Some(*code),
                Status::Running { waiters } => {
                    if !waiters.contains(&me) {
                        waiters.push(me);
                    }
                }
            }
        }
        // A wake-up between the unlock and here makes `block` return at
        // once, and the loop checks again.
        super::block();
    }
}

/// The exit code of process `id` if it has ended.
pub fn exit_status(id: ProcessId) -> Option<i64> {
    match TABLE.lock().get(&id)? {
        Status::Exited(code) => Some(*code),
        Status::Running { .. } => None,
    }
}
