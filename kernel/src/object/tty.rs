// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `Tty` kernel object: a terminal (`docs/linux.md` §5).
//!
//! ```text
//!   keyboard (InputAttach to the tty), or         program side ("slave")
//!   the master's TtyWrite, or PortSend INPUT_KEY  ─────────────────────
//!        │                                         Linux read(0)  ◄─┐
//!        ▼                                                          │
//!   line discipline (ldisc.rs): edit, echo, ^C ── ready bytes ──────┘
//!        │ echo
//!        ▼
//!   output ◄── Linux write(1), PortSend CONSOLE_WRITE (STELA programs)
//!        │
//!        ├─ no master: printed on the kernel console (kprint!)
//!        └─ master: buffered for the master's TtyRead (the terminal app)
//! ```
//!
//! A handle to the program side (`KernelObject::Tty`) works like a port
//! for `PortSend`: `CONSOLE_WRITE` messages are output, `INPUT_KEY`
//! messages are typed input. So a STELA program prints through a tty
//! given as its `[1]` without knowing it is one.

use super::ldisc::LineDiscipline;
use crate::task::{self, ProcessId, WaitQueue};
use abi::Error;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use frame::sync::SpinLock;

/// Most output bytes buffered for the master; more are dropped.
pub const OUTPUT_MAX: usize = 64 * 1024;

struct Inner {
    ldisc: LineDiscipline,
    /// Output waiting for the master.
    output: VecDeque<u8>,
    /// Threads waiting in a program-side read.
    readers: WaitQueue,
    /// Threads waiting in a master read.
    master_readers: WaitQueue,
    /// Linux processes that use the tty (interrupted by Ctrl-C).
    users: Vec<ProcessId>,
}

/// A terminal.
pub struct Tty {
    /// Whether a master end exists (else output is printed).
    has_master: bool,
    inner: SpinLock<Inner>,
}

impl Tty {
    /// A new terminal; with `master`, its output is kept for the master's
    /// reads instead of going to the kernel console.
    pub fn new(master: bool) -> Arc<Tty> {
        Arc::new(Tty {
            has_master: master,
            inner: SpinLock::new(Inner {
                ldisc: LineDiscipline::new(),
                output: VecDeque::new(),
                readers: WaitQueue::new(),
                master_readers: WaitQueue::new(),
                users: Vec::new(),
            }),
        })
    }

    /// Whether it has a master end.
    pub fn has_master(&self) -> bool {
        self.has_master
    }

    /// Program output (and echo).
    pub fn write(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if !self.has_master {
            for chunk in data.utf8_chunks() {
                frame::kprint!("{}", chunk.valid());
                if !chunk.invalid().is_empty() {
                    frame::kprint!("\u{FFFD}");
                }
            }
            return;
        }
        let waiters = {
            let mut inner = self.inner.lock();
            let room = OUTPUT_MAX.saturating_sub(inner.output.len());
            inner.output.extend(&data[..data.len().min(room)]);
            inner.master_readers.drain()
        };
        for id in waiters {
            task::wake(id);
        }
    }

    /// Typed input: one character through the line discipline.
    pub fn feed(&self, c: char) {
        let (fed, readers, users) = {
            let mut inner = self.inner.lock();
            let fed = inner.ldisc.feed(c);
            let readers = if fed.readable || fed.interrupt {
                inner.readers.drain()
            } else {
                Vec::new()
            };
            let users = if fed.interrupt {
                inner.users.clone()
            } else {
                Vec::new()
            };
            (fed, readers, users)
        };
        self.write(&fed.echo);
        let me = task::current_process().map(|p| p.id());
        for pid in users.into_iter().filter(|&p| Some(p) != me) {
            if let Some(process) = task::process::find(pid) {
                task::kill(&process, abi::EXIT_KILLED);
            }
        }
        for id in readers {
            task::wake(id);
        }
    }

    /// Typed input: every character of `text`.
    pub fn feed_str(&self, text: &str) {
        for c in text.chars() {
            self.feed(c);
        }
    }

    /// Registers a Linux process that uses the tty (Ctrl-C ends it).
    pub fn add_user(&self, pid: ProcessId) {
        let mut inner = self.inner.lock();
        inner
            .users
            .retain(|&p| task::process::exit_status(p).is_none());
        inner.users.push(pid);
    }

    /// Program-side read: up to `max` bytes (one line in canonical mode),
    /// waiting until something is readable or `deadline` (`None`:
    /// forever). An empty result is an end of file (Ctrl-D).
    pub fn read(&self, max: usize, deadline: Option<u64>) -> Result<Vec<u8>, Error> {
        let me = task::current_id();
        loop {
            if task::current_killed() {
                return Err(Error::NotSupported);
            }
            {
                let mut inner = self.inner.lock();
                if let Some(bytes) = inner.ldisc.read(max) {
                    return Ok(bytes);
                }
                if deadline.is_some_and(|d| frame::time::now_ns() >= d) {
                    return Err(Error::TimedOut);
                }
                inner.readers.add(me);
            }
            task::block_until(deadline);
            self.inner.lock().readers.remove(me);
        }
    }

    /// Whether a program-side read would return at once.
    pub fn readable(&self) -> bool {
        self.inner.lock().ldisc.readable()
    }

    /// Master read: up to `max` output bytes, waiting like [`Tty::read`].
    pub fn master_read(&self, max: usize, deadline: Option<u64>) -> Result<Vec<u8>, Error> {
        let me = task::current_id();
        loop {
            if task::current_killed() {
                return Err(Error::NotSupported);
            }
            {
                let mut inner = self.inner.lock();
                if !inner.output.is_empty() {
                    let n = inner.output.len().min(max);
                    return Ok(inner.output.drain(..n).collect());
                }
                if deadline.is_some_and(|d| frame::time::now_ns() >= d) {
                    return Err(Error::TimedOut);
                }
                inner.master_readers.add(me);
            }
            task::block_until(deadline);
            self.inner.lock().master_readers.remove(me);
        }
    }

    /// The line discipline's mode: (canonical, echo, signals).
    pub fn mode(&self) -> (bool, bool, bool) {
        let inner = self.inner.lock();
        (inner.ldisc.canonical, inner.ldisc.echo, inner.ldisc.signals)
    }

    /// Sets the line discipline's mode.
    pub fn set_mode(&self, canonical: bool, echo: bool, signals: bool) {
        let mut inner = self.inner.lock();
        inner.ldisc.canonical = canonical;
        inner.ldisc.echo = echo;
        inner.ldisc.signals = signals;
    }
}
