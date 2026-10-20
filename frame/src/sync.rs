// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Synchronisation primitives.
//!
//! Interrupt handlers take locks too (the input queue, the heap), so a lock
//! must never be held with interrupts enabled: if an interrupt arrived on the
//! same processor while the lock is held and its handler tried to take it,
//! the processor would wait for itself forever. [`SpinLock::lock`] therefore
//! disables interrupts first (`cli`) and the guard restores the previous
//! state when it is dropped. [`IrqGuard`] does only the interrupt half.
//!
//! On the host (unit tests) there are no interrupts to disable, so that half
//! is a no-op there.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Disables maskable interrupts and reports whether they were enabled
/// (RFLAGS.IF, bit 9; Intel SDM Vol. 1 3.4.3.3 "System Flags and IOPL
/// Field", Vol. 2A "CLI").
#[cfg(target_os = "none")]
fn irq_save() -> bool {
    crate::arch::x86_64::interrupts::save_and_disable()
}

/// Re-enables interrupts if `enabled` (the value [`irq_save`] returned).
#[cfg(target_os = "none")]
fn irq_restore(enabled: bool) {
    if enabled {
        crate::arch::x86_64::interrupts::enable();
    }
}

#[cfg(not(target_os = "none"))]
fn irq_save() -> bool {
    false
}

#[cfg(not(target_os = "none"))]
fn irq_restore(_enabled: bool) {}

/// Whether maskable interrupts are enabled on this processor (RFLAGS.IF).
#[cfg(target_os = "none")]
pub fn interrupts_enabled() -> bool {
    crate::arch::x86_64::interrupts::enabled()
}

/// Keeps maskable interrupts disabled on this processor while it exists;
/// dropping it restores the state from before [`IrqGuard::new`]. Guards
/// nest: only the outermost one turns interrupts back on.
///
/// It is neither `Send` nor `Sync`: the state belongs to the processor that
/// created it.
pub struct IrqGuard {
    was_enabled: bool,
    _not_send: PhantomData<*mut ()>,
}

impl IrqGuard {
    /// Disables interrupts.
    #[allow(clippy::new_without_default)]
    pub fn new() -> IrqGuard {
        IrqGuard {
            was_enabled: irq_save(),
            _not_send: PhantomData,
        }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        irq_restore(self.was_enabled);
    }
}

/// A mutual-exclusion lock that busy-waits, with interrupts disabled on
/// this processor while it is held.
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

    /// Disables interrupts, then waits until the lock is free and takes
    /// it. Interrupts stay disabled while spinning, so a lock held by an
    /// interrupted holder on this processor deadlocks; that is why every
    /// holder runs with interrupts off.
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let irq = IrqGuard::new();
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return SpinLockGuard {
                    lock: self,
                    _irq: irq,
                };
            }
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    /// Takes the lock if it is free (with interrupts disabled, as
    /// [`lock`](Self::lock)). On failure the interrupt state is unchanged.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        let irq = IrqGuard::new();
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard {
                lock: self,
                _irq: irq,
            })
    }
}

/// Access to the value of a locked [`SpinLock`]; unlocks when dropped, then
/// restores the interrupt state.
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    /// Dropped after `Drop::drop` releases the lock (fields drop after the
    /// struct's own `drop`), so interrupts come back only once it is free.
    _irq: IrqGuard,
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
    fn irq_guards_nest() {
        let outer = IrqGuard::new();
        let inner = IrqGuard::new();
        drop(inner);
        drop(outer);
        let lock = SpinLock::new(());
        let guard = lock.try_lock();
        assert!(guard.is_some());
        assert!(lock.try_lock().is_none());
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
