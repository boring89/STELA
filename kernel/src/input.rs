// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Input delivery: the `input-router` kernel thread takes the events the
//! keyboard interrupt queued in `frame::input` and sends each one, as an
//! `INPUT_KEY` message, to the port a program attached with `InputAttach`.
//!
//! ```text
//!  IRQ 1 ─► frame::input queue ─► notify hook: wake(input-router)
//!                                        │
//!               input-router: pop ───────┘──► port.send(INPUT_KEY)
//! ```
//!
//! The interrupt never touches a port; it only wakes the router. One
//! receiver at a time (a new attach replaces the old one); events are
//! dropped while nobody is attached or when the port is full.

use crate::object::port::{Port, QueuedMessage};
use crate::object::tty::Tty;
use crate::task;
use abi::{Error, Message, tag};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use frame::input::{self, InputEvent};
use frame::sync::SpinLock;

/// Where key events go.
#[derive(Clone)]
pub enum Target {
    /// `INPUT_KEY` messages to a port.
    Port(Arc<Port>),
    /// Typed into a terminal (the character each key makes).
    Tty(Arc<Tty>),
}

static TARGET: SpinLock<Option<Target>> = SpinLock::new(None);
/// The router's thread id (0 before `start`).
static ROUTER: AtomicU64 = AtomicU64::new(0);

/// Starts the router thread and hooks it to the input queue.
pub fn start() {
    match task::spawn("input-router", route) {
        Ok(id) => ROUTER.store(id, Ordering::Release),
        Err(e) => panic!("input: cannot start the router: {e:?}"),
    }
    input::set_notify(notify);
}

/// Called by `frame::input` after each event, inside the interrupt.
fn notify() {
    let id = ROUTER.load(Ordering::Acquire);
    if id != 0 {
        task::wake(id);
    }
}

/// The process that owns the screen (mapped the framebuffer: the window
/// server), 0 while nobody does.
static OWNER: AtomicU64 = AtomicU64::new(0);

/// The framebuffer was mapped by process `pid`: from now on only it may
/// attach (it hands the events to its clients).
pub fn set_owner(pid: crate::task::ProcessId) {
    OWNER.store(pid, Ordering::Release);
}

/// Whether process `pid` may `InputAttach`: anyone while no process owns
/// the screen (text mode) or after the owner ended; else only the owner.
pub fn may_attach(pid: crate::task::ProcessId) -> bool {
    let owner = OWNER.load(Ordering::Acquire);
    owner == 0 || owner == pid || crate::task::process::exit_status(owner).is_some()
}

/// `InputAttach`: events go to `target` from now on.
pub fn attach(target: Target) {
    *TARGET.lock() = Some(target);
}

fn route() {
    loop {
        while let Some(InputEvent::Key(event)) = input::pop() {
            let target = TARGET.lock().clone();
            match target {
                Some(Target::Port(port)) => deliver(port, &event.encode()),
                Some(Target::Tty(tty)) => {
                    if let Some(c) = abi::key::key_to_char(&event) {
                        tty.feed(c);
                    }
                }
                None => {}
            }
        }
        // An event pushed after the last pop leaves a pending wake-up, so
        // this returns at once.
        task::block();
    }
}

fn deliver(port: Arc<Port>, key: &[u8]) {
    let msg = Message::with_data(tag::INPUT_KEY, key);
    if port.send(QueuedMessage::plain(msg)) == Err(Error::PeerClosed) {
        let mut target = TARGET.lock();
        if matches!(&*target, Some(Target::Port(t)) if Arc::ptr_eq(t, &port)) {
            *target = None;
        }
    }
}
