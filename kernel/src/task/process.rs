// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Processes: an address space and the threads that run in it.
//!
//! A process ends when its last thread has been freed (reaped). Its exit
//! code is the one given to the first `ProcessExit` (or the kill, for a
//! fault), or 0 when the threads simply exited. The status stays in the
//! process table for [`wait_process`]; the address space is freed with the
//! last reference to the [`Process`], which only its threads hold.

use super::ThreadId;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
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
            let code = self.exit_code.lock().unwrap_or(0);
            finish(self.id, code);
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

/// Creates a process with an empty address space and no threads.
pub fn create(name: &str) -> Result<Arc<Process>, MapError> {
    let space = AddressSpace::new()?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
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
