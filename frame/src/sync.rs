// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Synchronisation primitives.
//!
//! P1 runs on one processor with interrupts disabled, so a lock is never
//! actually contended; [`SpinLock`] exists so that shared state already has
//! the right shape for SMP and interrupts. When interrupt handlers start
//! taking locks (P2), locking must also disable interrupts.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A mutual-exclusion lock that busy-waits.
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the lock hands out access to `value` to one holder at a time, so
// sharing the lock between threads is sound whenever moving `T` between
// threads is.
unsafe impl<T: Send> Sync for SpinLock<T> {}
// SAFETY: as above; owning the lock means owning `T`.
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// Creates an unlocked lock.
    pub const fn new(value: T) -> SpinLock<T> {
        SpinLock {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Waits until the lock is free and takes it.
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        loop {
            if let Some(guard) = self.try_lock() {
                return guard;
            }
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    /// Takes the lock if it is free.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard { lock: self })
    }
}

/// Access to the value of a locked [`SpinLock`]; unlocks when dropped.
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the guard proves this is the only holder of the lock.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard proves this is the only holder of the lock.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

/// A cell for statics that are written only during single-threaded boot
/// (descriptor tables, the TSS) and handed to the hardware by address.
///
/// It gives out raw pointers only; every access is `unsafe` and must argue
/// why no other reference is alive.
#[cfg(target_os = "none")]
pub(crate) struct RacyCell<T>(UnsafeCell<T>);

// SAFETY: users of RacyCell only access it during single-threaded
// initialisation (documented at each use), so no data race can occur.
#[cfg(target_os = "none")]
unsafe impl<T> Sync for RacyCell<T> {}

#[cfg(target_os = "none")]
impl<T> RacyCell<T> {
    /// Creates a cell.
    pub(crate) const fn new(value: T) -> RacyCell<T> {
        RacyCell(UnsafeCell::new(value))
    }

    /// A raw pointer to the contents.
    pub(crate) const fn get(&self) -> *mut T {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn lock_excludes_and_releases() {
        let lock = SpinLock::new(5);
        {
            let mut guard = lock.lock();
            *guard += 1;
            assert!(lock.try_lock().is_none());
        }
        assert_eq!(*lock.try_lock().unwrap(), 6);
    }

    #[test]
    fn lock_counts_across_threads() {
        let lock = Arc::new(SpinLock::new(0u64));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let lock = Arc::clone(&lock);
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        *lock.lock() += 1;
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(*lock.lock(), 40_000);
    }
}
