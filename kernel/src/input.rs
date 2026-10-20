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
use crate::task;
use abi::{Error, Message, tag};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use frame::input::{self, InputEvent};
use frame::sync::SpinLock;

static TARGET: SpinLock<Option<Arc<Port>>> = SpinLock::new(None);
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

/// `InputAttach`: events go to `port` from now on.
pub fn attach(port: Arc<Port>) {
    *TARGET.lock() = Some(port);
}

fn route() {
    loop {
        while let Some(InputEvent::Key(event)) = input::pop() {
            deliver(Message::with_data(tag::INPUT_KEY, &event.encode()));
        }
        // An event pushed after the last pop leaves a pending wake-up, so
        // this returns at once.
        task::block();
    }
}

fn deliver(msg: Message) {
    let Some(port) = TARGET.lock().clone() else {
        return;
    };
    if port.send(QueuedMessage::plain(msg)) == Err(Error::PeerClosed) {
        let mut target = TARGET.lock();
        if target.as_ref().is_some_and(|t| Arc::ptr_eq(t, &port)) {
            *target = None;
        }
    }
}
