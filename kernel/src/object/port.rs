// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ports: message queues with many senders and one receiving side.
//!
//! ```text
//!  sender ──PortSend──┐                       ┌──PortReceive── receiver
//!  sender ──PortSend──┼──► [msg][msg][msg] ───┤   (blocks while empty,
//!  kernel ──send──────┘     up to 64          │    up to a deadline)
//!                                             └── waiters: WaitQueue
//! ```
//!
//! A port belongs to the process that created it (or to the kernel). When
//! that process ends the port is closed: sends fail with `PeerClosed`,
//! receivers get the messages still queued and then `PeerClosed`. A
//! queued message owns the objects whose handles it carries; when it is
//! dropped unread, they are released.

use super::Entry;
use crate::task::{self, ProcessId, WaitQueue};
use abi::{Error, Message, PORT_QUEUE_DEPTH};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use frame::sync::SpinLock;

/// A message in a queue: the copy of the user's message and the objects
/// that left the sender's handle table with it.
pub struct QueuedMessage {
    /// The message (its handle slots are filled in on delivery).
    pub msg: Message,
    /// The objects carried, in slot order.
    pub objects: Vec<Entry>,
}

impl QueuedMessage {
    /// A message without handles.
    pub fn plain(msg: Message) -> QueuedMessage {
        let mut msg = msg;
        msg.handles = 0;
        QueuedMessage {
            msg,
            objects: Vec::new(),
        }
    }
}

struct Inner {
    queue: VecDeque<QueuedMessage>,
    waiters: WaitQueue,
    /// Threads waiting in a `WaitSet` that holds the port: all woken on
    /// every change.
    watchers: WaitQueue,
    closed: bool,
}

/// A port.
pub struct Port {
    owner: ProcessId,
    inner: SpinLock<Inner>,
}

/// The owner id of ports that belong to the kernel.
pub const KERNEL_OWNER: ProcessId = 0;

impl Port {
    /// A new, open, empty port owned by process `owner`
    /// ([`KERNEL_OWNER`] for the kernel).
    pub fn new(owner: ProcessId) -> Arc<Port> {
        Arc::new(Port {
            owner,
            inner: SpinLock::new(Inner {
                queue: VecDeque::new(),
                waiters: WaitQueue::new(),
                watchers: WaitQueue::new(),
                closed: false,
            }),
        })
    }

    /// The process the port belongs to.
    pub fn owner(&self) -> ProcessId {
        self.owner
    }

    /// Whether a send would be accepted now (open and not full).
    pub fn check_send(&self) -> Result<(), Error> {
        let inner = self.inner.lock();
        if inner.closed {
            Err(Error::PeerClosed)
        } else if inner.queue.len() >= PORT_QUEUE_DEPTH {
            Err(Error::Full)
        } else {
            Ok(())
        }
    }

    /// Queues `msg` and wakes one receiver. Fails with `PeerClosed` or
    /// `Full`; the message (and what it carries) is then dropped.
    pub fn send(&self, msg: QueuedMessage) -> Result<(), Error> {
        let (waiter, watchers) = {
            let mut inner = self.inner.lock();
            if inner.closed {
                Err(Error::PeerClosed)
            } else if inner.queue.len() >= PORT_QUEUE_DEPTH {
                Err(Error::Full)
            } else {
                inner.queue.push_back(msg);
                Ok((inner.waiters.pop(), inner.watchers.drain()))
            }
        }?;
        if let Some(id) = waiter {
            task::wake(id);
        }
        for id in watchers {
            task::wake(id);
        }
        Ok(())
    }

    /// Takes the oldest message, waiting until `deadline` (`now_ns`;
    /// `None`: forever). `room` is how many handles the receiver can take:
    /// a message carrying more stays queued (`OutOfHandles`).
    ///
    /// Errors: `TimedOut`; `PeerClosed` once the port is closed and empty;
    /// `NotSupported` if the caller's process was killed while waiting.
    pub fn receive(&self, deadline: Option<u64>, room: usize) -> Result<QueuedMessage, Error> {
        let me = task::current_id();
        loop {
            if task::current_killed() {
                return Err(Error::NotSupported);
            }
            {
                let mut inner = self.inner.lock();
                if let Some(front) = inner.queue.front() {
                    if front.objects.len() > room {
                        return Err(Error::OutOfHandles);
                    }
                    if let Some(msg) = inner.queue.pop_front() {
                        return Ok(msg);
                    }
                }
                if inner.closed {
                    return Err(Error::PeerClosed);
                }
                if deadline.is_some_and(|d| frame::time::now_ns() >= d) {
                    return Err(Error::TimedOut);
                }
                inner.waiters.add(me);
            }
            // A send between the unlock and here leaves a pending wake-up,
            // and block_until returns at once.
            task::block_until(deadline);
            self.inner.lock().waiters.remove(me);
            // Loop: the message may have been taken by another receiver,
            // the time may be up, or the process killed.
        }
    }

    /// Closes the port: later sends fail, the receivers are woken. The
    /// queued messages stay to be received.
    pub fn close(&self) {
        let (waiters, watchers) = {
            let mut inner = self.inner.lock();
            inner.closed = true;
            (inner.waiters.drain(), inner.watchers.drain())
        };
        for id in waiters.into_iter().chain(watchers) {
            task::wake(id);
        }
    }

    /// Closes the port and drops the queued messages (and the objects they
    /// carry).
    pub fn close_and_discard(&self) {
        self.close();
        let dropped = core::mem::take(&mut self.inner.lock().queue);
        drop(dropped);
    }

    /// Whether the port was closed.
    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }

    /// Messages waiting.
    pub fn queued(&self) -> usize {
        self.inner.lock().queue.len()
    }

    /// The `abi::wait` events that hold: `READABLE` with a message
    /// queued, `CLOSED` once closed (its owner ended), even with
    /// messages left.
    pub fn events(&self) -> u32 {
        let inner = self.inner.lock();
        let mut e = 0;
        if !inner.queue.is_empty() {
            e |= abi::wait::READABLE;
        }
        if inner.closed {
            e |= abi::wait::CLOSED;
        }
        e
    }

    /// Thread `id` (in a `WaitSet` wait) is woken by the next change.
    pub fn watch(&self, id: task::ThreadId) {
        self.inner.lock().watchers.add(id);
    }

    /// Undoes [`Port::watch`].
    pub fn unwatch(&self, id: task::ThreadId) {
        self.inner.lock().watchers.remove(id);
    }
}
