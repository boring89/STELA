// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A spin lock for user programs: threads that find it taken yield.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A value only one thread uses at a time.
pub struct SpinMutex<T> {
    taken: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: the value is reached only through a guard, and only one guard
// exists at a time (`taken`).
unsafe impl<T: Send> Sync for SpinMutex<T> {}

/// Access to the value; released when dropped.
pub struct SpinGuard<'a, T> {
    mutex: &'a SpinMutex<T>,
}

impl<T> SpinMutex<T> {
    /// A lock around `value`.
    pub const fn new(value: T) -> SpinMutex<T> {
        SpinMutex {
            taken: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Waits for the lock (yielding to other threads).
    pub fn lock(&self) -> SpinGuard<'_, T> {
        while self.taken.swap(true, Ordering::Acquire) {
            #[cfg(not(test))]
            crate::yield_now();
            #[cfg(test)]
            core::hint::spin_loop();
        }
        SpinGuard { mutex: self }
    }
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: this guard is the only one (see `Sync`).
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: this guard is the only one (see `Sync`).
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.taken.store(false, Ordering::Release);
    }
}
