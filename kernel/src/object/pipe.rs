// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pipes (P11, `docs/ipc.md` §14): a 64 KiB byte buffer with a reading
//! end and a writing end, each a kernel object of its own.
//!
//! ```text
//!  writer ──PipeWrite / PortSend CONSOLE_WRITE──► [ 64 KiB ] ──PipeRead──► reader
//!           waits while full                                    waits while empty
//!           PeerClosed once every reading handle is gone        0 bytes (end of
//!                                                               file) once every
//!                                                               writing handle is gone
//! ```
//!
//! Handles to one end share one `ReadEnd`/`WriteEnd` (duplicates clone
//! the `Arc`), so the end closes when its last handle does (`Drop`).

use crate::task::{self, WaitQueue};
use abi::{Error, wait};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use frame::sync::SpinLock;

/// Bytes a pipe holds.
pub const PIPE_BUFFER: usize = abi::PIPE_BUFFER;

struct Inner {
    buf: VecDeque<u8>,
    reader: bool,
    writer: bool,
    readers: WaitQueue,
    writers: WaitQueue,
    watchers: WaitQueue,
}

/// The shared buffer.
pub struct Pipe {
    inner: SpinLock<Inner>,
}

/// The reading end (what a `PIPE_READ` handle refers to).
pub struct ReadEnd(pub Arc<Pipe>);
/// The writing end.
pub struct WriteEnd(pub Arc<Pipe>);

impl Drop for ReadEnd {
    fn drop(&mut self) {
        self.0.close(true);
    }
}

impl Drop for WriteEnd {
    fn drop(&mut self) {
        self.0.close(false);
    }
}

impl Pipe {
    /// A new pipe's two ends.
    pub fn create() -> (Arc<ReadEnd>, Arc<WriteEnd>) {
        let pipe = Arc::new(Pipe {
            inner: SpinLock::new(Inner {
                buf: VecDeque::new(),
                reader: true,
                writer: true,
                readers: WaitQueue::new(),
                writers: WaitQueue::new(),
                watchers: WaitQueue::new(),
            }),
        });
        (
            Arc::new(ReadEnd(Arc::clone(&pipe))),
            Arc::new(WriteEnd(pipe)),
        )
    }

    fn wake(ids: Vec<u64>) {
        for id in ids {
            task::wake(id);
        }
    }

    fn close(&self, reading: bool) {
        let woken = {
            let mut i = self.inner.lock();
            if reading {
                i.reader = false;
            } else {
                i.writer = false;
            }
            let mut w = i.readers.drain();
            w.extend(i.writers.drain());
            w.extend(i.watchers.drain());
            w
        };
        Pipe::wake(woken);
    }

    /// Up to `max` bytes, waiting until there are some or `deadline`
    /// (`None`: forever); empty at the end (no writer and nothing left).
    /// `TimedOut`; `NotSupported` when the caller was killed.
    pub fn read(&self, max: usize, deadline: Option<u64>) -> Result<Vec<u8>, Error> {
        let me = task::current_id();
        loop {
            if task::current_killed() {
                return Err(Error::NotSupported);
            }
            {
                let mut i = self.inner.lock();
                if !i.buf.is_empty() || !i.writer {
                    let n = i.buf.len().min(max);
                    let data: Vec<u8> = i.buf.drain(..n).collect();
                    let mut woken = i.writers.drain();
                    woken.extend(i.watchers.drain());
                    drop(i);
                    Pipe::wake(woken);
                    return Ok(data);
                }
                if deadline.is_some_and(|d| frame::time::now_ns() >= d) {
                    return Err(Error::TimedOut);
                }
                i.readers.add(me);
            }
            task::block_until(deadline);
            self.inner.lock().readers.remove(me);
        }
    }

    /// Writes as much of `data` as fits, waiting for room until
    /// `deadline`; at least one byte unless `data` is empty.
    /// `PeerClosed` when no reader is left; `TimedOut`.
    pub fn write(&self, data: &[u8], deadline: Option<u64>) -> Result<usize, Error> {
        let me = task::current_id();
        loop {
            if task::current_killed() {
                return Err(Error::NotSupported);
            }
            {
                let mut i = self.inner.lock();
                if !i.reader {
                    return Err(Error::PeerClosed);
                }
                let room = PIPE_BUFFER - i.buf.len();
                if room > 0 || data.is_empty() {
                    let n = room.min(data.len());
                    i.buf.extend(&data[..n]);
                    let mut woken = i.readers.drain();
                    woken.extend(i.watchers.drain());
                    drop(i);
                    Pipe::wake(woken);
                    return Ok(n);
                }
                if deadline.is_some_and(|d| frame::time::now_ns() >= d) {
                    return Err(Error::TimedOut);
                }
                i.writers.add(me);
            }
            task::block_until(deadline);
            self.inner.lock().writers.remove(me);
        }
    }

    /// Writes all of `data`, waiting as long as it takes.
    pub fn write_all(&self, data: &[u8]) -> Result<(), Error> {
        let mut done = 0;
        while done < data.len() {
            done += self.write(&data[done..], None)?;
        }
        Ok(())
    }

    /// The `abi::wait` events of the reading end (`reading`) or the
    /// writing end.
    pub fn events(&self, reading: bool) -> u32 {
        let i = self.inner.lock();
        if reading {
            let mut e = 0;
            if !i.buf.is_empty() || !i.writer {
                e |= wait::READABLE;
            }
            if !i.writer {
                e |= wait::CLOSED;
            }
            e
        } else if !i.reader {
            wait::CLOSED
        } else if i.buf.len() < PIPE_BUFFER {
            wait::WRITABLE
        } else {
            0
        }
    }

    /// Bytes waiting to be read.
    pub fn len(&self) -> usize {
        self.inner.lock().buf.len()
    }

    /// Whether nothing waits to be read.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Thread `id` (in a `WaitSet` wait) is woken by the next change.
    pub fn watch(&self, id: task::ThreadId) {
        self.inner.lock().watchers.add(id);
    }

    /// Undoes [`Pipe::watch`].
    pub fn unwatch(&self, id: task::ThreadId) {
        self.inner.lock().watchers.remove(id);
    }
}
