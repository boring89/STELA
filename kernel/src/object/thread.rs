// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a thread handle refers to (P11, `docs/process.md` §8.1): the
//! thread's end, kept apart from the thread itself so that the handle
//! outlives it.
//!
//! ```text
//!  ThreadCreate ──► Thread { exit: Arc<ThreadExit> } ──► handle (WAIT)
//!                       │ ThreadExit(value): value noted
//!                       ▼ zombie, freed by reap()
//!                   ThreadExit::finish() ──► ThreadJoin / WaitSet wake up
//! ```
//!
//! The record is finished when the scheduler frees the thread (`reap`),
//! after its last switch: by then nothing runs on its user stack any more,
//! so a joiner may reuse that stack.

use crate::task::{self, WaitQueue};
use abi::{Error, wait};
use alloc::vec::Vec;
use frame::sync::SpinLock;

struct Inner {
    /// The value given to `ThreadExit`, if the thread called it.
    value: Option<u64>,
    finished: bool,
    joiners: WaitQueue,
    watchers: WaitQueue,
}

/// A thread's end.
pub struct ThreadExit {
    inner: SpinLock<Inner>,
}

impl ThreadExit {
    /// The record of a thread that has not ended.
    pub fn new() -> ThreadExit {
        ThreadExit {
            inner: SpinLock::new(Inner {
                value: None,
                finished: false,
                joiners: WaitQueue::new(),
                watchers: WaitQueue::new(),
            }),
        }
    }

    /// The thread is about to end with `value`.
    pub fn set_value(&self, value: u64) {
        self.inner.lock().value = Some(value);
    }

    /// The thread is gone: wakes the joiners and the watchers.
    pub fn finish(&self) {
        let woken: Vec<u64> = {
            let mut i = self.inner.lock();
            i.finished = true;
            let mut w = i.joiners.drain();
            w.extend(i.watchers.drain());
            w
        };
        for id in woken {
            task::wake(id);
        }
    }

    /// The exit value once the thread is gone: what it gave `ThreadExit`,
    /// `EXIT_KILLED` if it ended with its process.
    fn result(i: &Inner) -> Option<u64> {
        i.finished
            .then(|| i.value.unwrap_or(abi::EXIT_KILLED as u64))
    }

    /// Waits until the thread is gone or `deadline` (`None`: forever).
    /// `TimedOut`; `NotSupported` when the caller was killed.
    pub fn join(&self, deadline: Option<u64>) -> Result<u64, Error> {
        let me = task::current_id();
        loop {
            if task::current_killed() {
                return Err(Error::NotSupported);
            }
            {
                let mut i = self.inner.lock();
                if let Some(value) = ThreadExit::result(&i) {
                    return Ok(value);
                }
                if deadline.is_some_and(|d| frame::time::now_ns() >= d) {
                    return Err(Error::TimedOut);
                }
                i.joiners.add(me);
            }
            task::block_until(deadline);
            self.inner.lock().joiners.remove(me);
        }
    }

    /// The `abi::wait` events: `EXITED` once the thread is gone.
    pub fn events(&self) -> u32 {
        if self.inner.lock().finished {
            wait::EXITED
        } else {
            0
        }
    }

    /// Thread `id` (in a `WaitSet` wait) is woken when the thread ends.
    pub fn watch(&self, id: task::ThreadId) {
        self.inner.lock().watchers.add(id);
    }

    /// Undoes [`ThreadExit::watch`].
    pub fn unwatch(&self, id: task::ThreadId) {
        self.inner.lock().watchers.remove(id);
    }
}

impl Default for ThreadExit {
    fn default() -> ThreadExit {
        ThreadExit::new()
    }
}
