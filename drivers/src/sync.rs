// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Waiting in drivers. A disk request takes milliseconds, so the thread
//! that made it sleeps until the interrupt arrives instead of spinning.
//! The scheduler lives in the kernel crate, which depends on this one, so
//! the kernel registers its functions here ([`set_hooks`]); before that
//! (and on the host) waiting falls back to spinning.
//!
//! [`SleepLock`] serialises requests to one device: unlike `SpinLock` it
//! can be held while sleeping (interrupts stay enabled).

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicBool, Ordering};
use frame::sync::{Once, SpinLock};

/// The scheduler functions drivers use.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// The running thread's id.
    pub current: fn() -> u64,
    /// Blocks the caller until woken or until the deadline (`now_ns`);
    /// returns whether it was woken.
    pub block_until: fn(Option<u64>) -> bool,
    /// Wakes a thread (callable from an interrupt handler).
    pub wake: fn(u64),
    /// Nanoseconds since boot.
    pub now_ns: fn() -> u64,
}

static HOOKS: Once<Hooks> = Once::new();

/// Registers the kernel's scheduler functions. Only the first call counts.
pub fn set_hooks(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

/// The hooks, once registered.
pub fn hooks() -> Option<&'static Hooks> {
    HOOKS.get()
}

/// Waits until `done()` holds or `timeout_ns` passed; returns `done()`'s
/// final answer. `register(id)` is called with the caller's thread id
/// before the first sleep, so that an interrupt handler can wake it; the
/// condition is checked again after every wake-up and at least every
/// 10 ms (a lost interrupt costs time, not correctness).
pub fn wait_for(timeout_ns: u64, register: impl Fn(u64), done: impl Fn() -> bool) -> bool {
    let Some(h) = hooks() else {
        // No scheduler: spin (bounded by a loop count, as there may be no
        // clock either).
        register(0);
        for _ in 0..50_000_000u64 {
            if done() {
                return true;
            }
            core::hint::spin_loop();
        }
        return done();
    };
    let deadline = (h.now_ns)().saturating_add(timeout_ns);
    register((h.current)());
    loop {
        if done() {
            return true;
        }
        let now = (h.now_ns)();
        if now >= deadline {
            return done();
        }
        (h.block_until)(Some(now.saturating_add(10_000_000).min(deadline)));
    }
}

/// Wakes thread `id` (no-op before the hooks are set or for id 0).
pub fn wake(id: u64) {
    if id != 0
        && let Some(h) = hooks()
    {
        (h.wake)(id);
    }
}

/// A lock that may be held across sleeps. Waiters sleep (with the hooks)
/// or spin (without).
pub struct SleepLock {
    busy: AtomicBool,
    waiters: SpinLock<VecDeque<u64>>,
}

/// Holding a [`SleepLock`]; releases it when dropped.
pub struct SleepLockGuard<'a> {
    lock: &'a SleepLock,
}

impl SleepLock {
    /// An unlocked lock.
    pub const fn new() -> SleepLock {
        SleepLock {
            busy: AtomicBool::new(false),
            waiters: SpinLock::new(VecDeque::new()),
        }
    }

    /// Takes the lock, sleeping while someone else holds it.
    pub fn lock(&self) -> SleepLockGuard<'_> {
        loop {
            if !self.busy.swap(true, Ordering::Acquire) {
                return SleepLockGuard { lock: self };
            }
            match hooks() {
                Some(h) => {
                    let me = (h.current)();
                    self.waiters.lock().push_back(me);
                    // Released between the swap and the push: try again
                    // (the release may also have woken us: harmless).
                    if self.busy.load(Ordering::Acquire) {
                        (h.block_until)(None);
                    }
                    self.waiters.lock().retain(|&id| id != me);
                }
                None => core::hint::spin_loop(),
            }
        }
    }
}

impl Default for SleepLock {
    fn default() -> SleepLock {
        SleepLock::new()
    }
}

impl Drop for SleepLockGuard<'_> {
    fn drop(&mut self) {
        self.lock.busy.store(false, Ordering::Release);
        let next = self.lock.waiters.lock().pop_front();
        if let Some(id) = next {
            wake(id);
        }
    }
}

/// Spins until `done()` holds or `timeout_ns` passed (by `frame::time`,
/// so this needs the timer running); for short waits during device setup.
/// Returns `done()`'s final answer.
#[cfg(target_os = "none")]
pub fn spin_for(timeout_ns: u64, done: impl Fn() -> bool) -> bool {
    let deadline = frame::time::now_ns().saturating_add(timeout_ns);
    while frame::time::now_ns() < deadline {
        if done() {
            return true;
        }
        core::hint::spin_loop();
    }
    done()
}

/// A value behind a [`SleepLock`]: [`SleepMutex::with`] may sleep (do
/// I/O) while it works on the value. The value is moved out of its slot
/// for the duration, so no `unsafe` is needed.
pub struct SleepMutex<T> {
    lock: SleepLock,
    slot: SpinLock<Option<T>>,
}

impl<T> SleepMutex<T> {
    /// A mutex holding `value`.
    pub const fn new(value: T) -> SleepMutex<T> {
        SleepMutex {
            lock: SleepLock::new(),
            slot: SpinLock::new(Some(value)),
        }
    }

    /// Runs `f` on the value, alone.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let _guard = self.lock.lock();
        let taken = self.slot.lock().take();
        let Some(mut value) = taken else {
            panic!("sync: SleepMutex value missing (a panic inside with?)");
        };
        let result = f(&mut value);
        *self.slot.lock() = Some(value);
        result
    }
}
