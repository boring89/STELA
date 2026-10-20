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
        let never_ran = matches!(
            TABLE.lock().get(&self.id).map(|r| &r.status),
            Some(Status::Running { .. })
        );
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

/// A process table record.
struct Record {
    name: String,
    /// The process while it exists (its threads hold it).
    process: Weak<Process>,
    status: Status,
}

static TABLE: SpinLock<BTreeMap<ProcessId, Record>> = SpinLock::new(BTreeMap::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The bootstrap handles `[1]`, `[2]`, `[3]` of a new process.
pub type Initial = [Option<Entry>; 3];

/// The usual `[1]`..`[3]`: the console service (if it runs), `bootstrap`,
/// the root of `Data` (if mounted).
pub fn standard_initial(bootstrap: Option<Entry>) -> Initial {
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
    [console, bootstrap, cwd]
}

/// Creates a process with an empty address space, no threads and the
/// bootstrap handles: `[0]` its main port, `[1]` the console service (if
/// it runs), `[2]` nothing, `[3]` the root of `Data` (if mounted).
pub fn create(name: &str) -> Result<Arc<Process>, MapError> {
    create_with(name, standard_initial(None))
}

/// Like [`create`], with `initial` as handles `[1]`..`[3]`.
pub fn create_with(name: &str, initial: Initial) -> Result<Arc<Process>, MapError> {
    let space = AddressSpace::new()?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let main_port = Port::new(id);
    let main = Entry {
        object: KernelObject::Port(Arc::clone(&main_port)),
        rights: Rights::RECEIVE | Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
    };
    let [console, bootstrap, cwd] = initial;
    let handles = HandleTable::with_initial([Some(main), console, bootstrap, cwd]);
    let process = Arc::new(Process {
        id,
        name: String::from(name),
        space,
        live: AtomicUsize::new(0),
        exit_code: SpinLock::new(None),
        handles: SpinLock::new(handles),
        main_port,
        ports: SpinLock::new(Vec::new()),
        maps: SpinLock::new(Regions::new(MAP_BASE, MAP_END)),
    });
    TABLE.lock().insert(
        id,
        Record {
            name: String::from(name),
            process: Arc::downgrade(&process),
            status: Status::Running {
                waiters: Vec::new(),
            },
        },
    );
    Ok(process)
}

/// Records the end of process `id` and wakes its waiters.
fn finish(id: ProcessId, code: i64) {
    let waiters = {
        let mut table = TABLE.lock();
        match table.get_mut(&id) {
            Some(record) => match core::mem::replace(&mut record.status, Status::Exited(code)) {
                Status::Running { waiters } => waiters,
                Status::Exited(_) => Vec::new(),
            },
            None => Vec::new(),
        }
    };
    for waiter in waiters {
        super::wake(waiter);
    }
}

/// Why [`wait_until`] returned without an exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitError {
    /// No such process.
    Unknown,
    /// The deadline passed.
    TimedOut,
    /// The waiting thread's process was killed.
    Killed,
}

/// Waits until process `id` has ended (or `deadline`, `now_ns`; `None`:
/// forever) and returns its exit code. The status stays, so waiting again
/// returns it again.
pub fn wait_until(id: ProcessId, deadline: Option<u64>) -> Result<i64, WaitError> {
    let me = super::current_id();
    loop {
        if super::current_killed() {
            return Err(WaitError::Killed);
        }
        {
            let mut table = TABLE.lock();
            let record = table.get_mut(&id).ok_or(WaitError::Unknown)?;
            match &mut record.status {
                Status::Exited(code) => return Ok(*code),
                Status::Running { waiters } => {
                    if deadline.is_some_and(|d| frame::time::now_ns() >= d) {
                        waiters.retain(|&w| w != me);
                        return Err(WaitError::TimedOut);
                    }
                    if !waiters.contains(&me) {
                        waiters.push(me);
                    }
                }
            }
        }
        // A wake-up between the unlock and here makes this return at
        // once, and the loop checks again.
        super::block_until(deadline);
    }
}

/// Waits until process `id` has ended and returns its exit code (`None`
/// for an unknown id).
pub fn wait_process(id: ProcessId) -> Option<i64> {
    wait_until(id, None).ok()
}

/// The exit code of process `id` if it has ended.
pub fn exit_status(id: ProcessId) -> Option<i64> {
    match TABLE.lock().get(&id)?.status {
        Status::Exited(code) => Some(code),
        Status::Running { .. } => None,
    }
}

/// The process `id` while it exists.
pub fn find(id: ProcessId) -> Option<Arc<Process>> {
    TABLE.lock().get(&id)?.process.upgrade()
}

/// One line of the process list: (id, name, live threads, exit code once
/// ended).
pub fn list() -> Vec<(ProcessId, String, usize, Option<i64>)> {
    // The processes are looked at outside the table's lock: dropping the
    // last reference to one takes the lock (Drop).
    let records: Vec<_> = TABLE
        .lock()
        .iter()
        .map(|(&id, r)| {
            let code = match r.status {
                Status::Exited(code) => Some(code),
                Status::Running { .. } => None,
            };
            (id, r.name.clone(), r.process.clone(), code)
        })
        .collect();
    records
        .into_iter()
        .map(|(id, name, weak, code)| {
            let threads = weak.upgrade().map_or(0, |p| p.live.load(Ordering::Relaxed));
            (id, name, threads, code)
        })
        .collect()
}
