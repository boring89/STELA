// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Wait sets: waiting on several objects at once (`docs/ipc.md` §13).
//!
//! ```text
//!  WaitSetWait ─► loop:
//!     watch every member   (the thread joins each object's watchers)
//!     look at every member (events in its mask that hold now)
//!     some hold → unwatch all, return them
//!     deadline passed → unwatch all, TimedOut
//!     block_until(deadline)  ← any member's change wakes every watcher
//!     unwatch all
//! ```
//!
//! Watching before looking means a change between the look and the
//! block leaves a pending wake-up, so `block_until` returns at once.
//! Members are held weakly: a set does not keep a socket open or a port
//! alive; a member whose object went away is dropped from the set.

use super::KernelObject;
use super::port::Port;
use super::tty::Tty;
use crate::task::{self, ProcessId};
use abi::Error;
use abi::wait::{self, Ready};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use frame::sync::SpinLock;

/// What a member refers to (weakly).
#[derive(Clone)]
enum Target {
    Port(Weak<Port>),
    Tty(Weak<Tty>),
    TtyMaster(Weak<Tty>),
    Socket(Weak<crate::net::Socket>),
    Process(ProcessId),
    Audio(Weak<crate::audio::AudioSession>),
}

impl Target {
    fn of(object: &KernelObject) -> Result<Target, Error> {
        Ok(match object {
            KernelObject::Port(p) => Target::Port(Arc::downgrade(p)),
            KernelObject::Tty(t) => Target::Tty(Arc::downgrade(t)),
            KernelObject::TtyMaster(t) => Target::TtyMaster(Arc::downgrade(t)),
            KernelObject::Socket(s) => Target::Socket(Arc::downgrade(s)),
            KernelObject::Process(id) => Target::Process(*id),
            KernelObject::Audio(a) => Target::Audio(Arc::downgrade(a)),
            _ => return Err(Error::WrongType),
        })
    }

    fn same(&self, other: &Target) -> bool {
        match (self, other) {
            (Target::Port(a), Target::Port(b)) => a.ptr_eq(b),
            (Target::Tty(a), Target::Tty(b)) => a.ptr_eq(b),
            (Target::TtyMaster(a), Target::TtyMaster(b)) => a.ptr_eq(b),
            (Target::Socket(a), Target::Socket(b)) => a.ptr_eq(b),
            (Target::Process(a), Target::Process(b)) => a == b,
            (Target::Audio(a), Target::Audio(b)) => a.ptr_eq(b),
            _ => false,
        }
    }

    /// The events that hold now; `None` once the object is gone.
    fn events(&self) -> Option<u32> {
        Some(match self {
            Target::Port(p) => p.upgrade()?.events(),
            Target::Tty(t) => {
                if t.upgrade()?.readable() {
                    wait::READABLE
                } else {
                    0
                }
            }
            Target::TtyMaster(t) => {
                let t = t.upgrade()?;
                wait::WRITABLE
                    | if t.master_readable() {
                        wait::READABLE
                    } else {
                        0
                    }
            }
            Target::Socket(s) => crate::net::events(&*s.upgrade()?),
            Target::Process(id) => {
                if task::process::exit_status(*id).is_some() {
                    wait::EXITED
                } else {
                    0
                }
            }
            Target::Audio(a) => {
                a.upgrade()?;
                crate::audio::events()
            }
        })
    }

    fn watch(&self, me: task::ThreadId, on: bool) {
        match self {
            Target::Port(p) => {
                if let Some(p) = p.upgrade() {
                    if on { p.watch(me) } else { p.unwatch(me) }
                }
            }
            Target::Tty(t) | Target::TtyMaster(t) => {
                if let Some(t) = t.upgrade() {
                    if on { t.watch(me) } else { t.unwatch(me) }
                }
            }
            Target::Socket(_) => {
                if on {
                    crate::net::watch(me)
                } else {
                    crate::net::unwatch(me)
                }
            }
            Target::Process(id) => {
                if on {
                    task::process::watch(*id, me)
                } else {
                    task::process::unwatch(*id, me)
                }
            }
            Target::Audio(_) => {
                if on {
                    crate::audio::watch(me)
                } else {
                    crate::audio::unwatch(me)
                }
            }
        }
    }
}

#[derive(Clone)]
struct Member {
    target: Target,
    key: u64,
    mask: u32,
}

/// A set of objects to wait on together.
pub struct WaitSet {
    members: SpinLock<Vec<Member>>,
}

impl WaitSet {
    /// An empty set.
    pub fn new() -> Arc<WaitSet> {
        Arc::new(WaitSet {
            members: SpinLock::new(Vec::new()),
        })
    }

    /// Adds `object` (or updates its key and mask if it is a member);
    /// `mask` 0 means every event. `WrongType` for objects that cannot be
    /// waited on, `Full` past `wait::MAX_MEMBERS`, `InvalidArgument` for
    /// undefined event bits.
    pub fn add(&self, object: &KernelObject, key: u64, mask: u32) -> Result<(), Error> {
        if mask & !wait::ALL != 0 {
            return Err(Error::InvalidArgument);
        }
        let mask = if mask == 0 { wait::ALL } else { mask };
        let target = Target::of(object)?;
        let mut members = self.members.lock();
        if let Some(m) = members.iter_mut().find(|m| m.target.same(&target)) {
            m.key = key;
            m.mask = mask;
            return Ok(());
        }
        if members.len() >= wait::MAX_MEMBERS {
            return Err(Error::Full);
        }
        members.push(Member { target, key, mask });
        Ok(())
    }

    /// Removes `object`; `NotFound` if it is not a member.
    pub fn remove(&self, object: &KernelObject) -> Result<(), Error> {
        let target = Target::of(object)?;
        let mut members = self.members.lock();
        let i = members
            .iter()
            .position(|m| m.target.same(&target))
            .ok_or(Error::NotFound)?;
        members.remove(i);
        Ok(())
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.members.lock().len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The members ready now (at most `max`), dropping the ones whose
    /// object is gone.
    fn scan(&self, members: &[Member], max: usize) -> (Vec<Ready>, bool) {
        let mut ready = Vec::new();
        let mut gone = false;
        for m in members {
            match m.target.events() {
                None => gone = true,
                Some(e) if e & m.mask != 0 && ready.len() < max => ready.push(Ready {
                    key: m.key,
                    events: e & m.mask,
                }),
                Some(_) => {}
            }
        }
        (ready, gone)
    }

    /// Waits until some member is ready or `deadline` (`now_ns`; `None`:
    /// forever) and returns up to `max` (≥ 1) ready members in set order.
    /// `TimedOut`; `NotSupported` if the caller's process was killed.
    pub fn wait(&self, deadline: Option<u64>, max: usize) -> Result<Vec<Ready>, Error> {
        let me = task::current_id();
        let max = max.max(1);
        loop {
            if task::current_killed() {
                return Err(Error::NotSupported);
            }
            let members: Vec<Member> = self.members.lock().clone();
            for m in &members {
                m.target.watch(me, true);
            }
            let (ready, gone) = self.scan(&members, max);
            if gone {
                self.members.lock().retain(|m| m.target.events().is_some());
            }
            let expired = deadline.is_some_and(|d| frame::time::now_ns() >= d);
            if ready.is_empty() && !expired {
                task::block_until(deadline);
            }
            for m in &members {
                m.target.watch(me, false);
            }
            if !ready.is_empty() {
                return Ok(ready);
            }
            if expired {
                return Err(Error::TimedOut);
            }
        }
    }
}
