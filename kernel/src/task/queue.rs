// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The scheduler's containers, as pure logic: the run queue (first in,
//! first out), the sleep list (ordered by wake-up time) and the wait
//! queue (threads blocked on one object, such as a port).

use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Threads ready to run, in round-robin order.
#[derive(Debug)]
pub struct RunQueue<T> {
    items: VecDeque<T>,
}

impl<T> RunQueue<T> {
    /// An empty queue.
    pub const fn new() -> RunQueue<T> {
        RunQueue {
            items: VecDeque::new(),
        }
    }

    /// Adds `item` at the back.
    pub fn push(&mut self, item: T) {
        self.items.push_back(item);
    }

    /// Takes the item at the front.
    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The items, front first.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.items.iter_mut()
    }

    /// Removes and returns every item for which `pred` holds, keeping the
    /// order of the others.
    pub fn take_where(&mut self, mut pred: impl FnMut(&T) -> bool) -> Vec<T> {
        let mut taken = Vec::new();
        let mut kept = VecDeque::with_capacity(self.items.len());
        for item in self.items.drain(..) {
            if pred(&item) {
                taken.push(item);
            } else {
                kept.push_back(item);
            }
        }
        self.items = kept;
        taken
    }
}

impl<T> Default for RunQueue<T> {
    fn default() -> RunQueue<T> {
        RunQueue::new()
    }
}

/// Sleeping threads ordered by wake-up time; equal times keep their
/// insertion order.
#[derive(Debug)]
pub struct SleepList<T> {
    /// Sorted by the first element, earliest first.
    items: VecDeque<(u64, T)>,
}

impl<T> SleepList<T> {
    /// An empty list.
    pub const fn new() -> SleepList<T> {
        SleepList {
            items: VecDeque::new(),
        }
    }

    /// Adds `item`, to be woken at `wake_at`.
    pub fn insert(&mut self, wake_at: u64, item: T) {
        // After every entry with the same time: first come, first woken.
        let at = self.items.partition_point(|(t, _)| *t <= wake_at);
        self.items.insert(at, (wake_at, item));
    }

    /// Takes the earliest item if its time is `<= now`.
    pub fn pop_expired(&mut self, now: u64) -> Option<T> {
        match self.items.front() {
            Some((t, _)) if *t <= now => self.items.pop_front().map(|(_, item)| item),
            _ => None,
        }
    }

    /// Whether some item satisfies `pred`.
    pub fn any(&self, mut pred: impl FnMut(&T) -> bool) -> bool {
        self.items.iter().any(|(_, item)| pred(item))
    }

    /// The earliest wake-up time.
    pub fn next_wake(&self) -> Option<u64> {
        self.items.front().map(|(t, _)| *t)
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Removes and returns every item for which `pred` holds.
    pub fn take_where(&mut self, mut pred: impl FnMut(&T) -> bool) -> Vec<T> {
        let mut taken = Vec::new();
        let mut kept = VecDeque::with_capacity(self.items.len());
        for (t, item) in self.items.drain(..) {
            if pred(&item) {
                taken.push(item);
            } else {
                kept.push_back((t, item));
            }
        }
        self.items = kept;
        taken
    }
}

impl<T> Default for SleepList<T> {
    fn default() -> SleepList<T> {
        SleepList::new()
    }
}

/// The ids of the threads waiting for one object, first come first
/// served. A thread is in it at most once.
#[derive(Debug, Default)]
pub struct WaitQueue {
    ids: VecDeque<u64>,
}

impl WaitQueue {
    /// An empty queue.
    pub const fn new() -> WaitQueue {
        WaitQueue {
            ids: VecDeque::new(),
        }
    }

    /// Adds thread `id` at the back, unless it is already waiting.
    pub fn add(&mut self, id: u64) {
        if !self.ids.contains(&id) {
            self.ids.push_back(id);
        }
    }

    /// Takes the thread that has waited longest.
    pub fn pop(&mut self) -> Option<u64> {
        self.ids.pop_front()
    }

    /// Removes thread `id` (it stopped waiting: woken by someone else, or
    /// timed out). Returns whether it was there.
    pub fn remove(&mut self, id: u64) -> bool {
        match self.ids.iter().position(|&i| i == id) {
            Some(at) => {
                self.ids.remove(at);
                true
            }
            None => false,
        }
    }

    /// Takes every waiting thread, longest waiting first.
    pub fn drain(&mut self) -> Vec<u64> {
        self.ids.drain(..).collect()
    }

    /// Number of waiting threads.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether nobody waits.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_queue_is_fifo() {
        let mut q = RunQueue::new();
        assert!(q.is_empty());
        q.push(1);
        q.push(2);
        q.push(3);
        assert_eq!(q.len(), 3);
        assert_eq!(q.pop(), Some(1));
        q.push(4);
        assert_eq!(q.pop(), Some(2));
        assert_eq!(q.pop(), Some(3));
        assert_eq!(q.pop(), Some(4));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn run_queue_round_robin() {
        // Popping the front and pushing it back cycles through everyone.
        let mut q = RunQueue::new();
        for i in 0..3 {
            q.push(i);
        }
        let mut order = Vec::new();
        for _ in 0..6 {
            let i = q.pop().unwrap();
            order.push(i);
            q.push(i);
        }
        assert_eq!(order, [0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn run_queue_take_where_keeps_order() {
        let mut q = RunQueue::new();
        for i in 0..6 {
            q.push(i);
        }
        assert_eq!(q.take_where(|i| i % 2 == 1), [1, 3, 5]);
        assert_eq!(q.pop(), Some(0));
        assert_eq!(q.pop(), Some(2));
        assert_eq!(q.pop(), Some(4));
        assert!(q.is_empty());
    }

    #[test]
    fn sleep_list_orders_by_time() {
        let mut s = SleepList::new();
        s.insert(30, "c");
        s.insert(10, "a");
        s.insert(20, "b");
        assert_eq!(s.len(), 3);
        assert_eq!(s.next_wake(), Some(10));
        assert_eq!(s.pop_expired(9), None);
        assert_eq!(s.pop_expired(10), Some("a"));
        assert_eq!(s.pop_expired(25), Some("b"));
        assert_eq!(s.pop_expired(25), None);
        assert_eq!(s.pop_expired(u64::MAX), Some("c"));
        assert!(s.is_empty());
        assert_eq!(s.next_wake(), None);
    }

    #[test]
    fn sleep_list_ties_are_fifo() {
        let mut s = SleepList::new();
        s.insert(5, 1);
        s.insert(5, 2);
        s.insert(1, 0);
        s.insert(5, 3);
        let woken: Vec<_> = core::iter::from_fn(|| s.pop_expired(5)).collect();
        assert_eq!(woken, [0, 1, 2, 3]);
    }

    #[test]
    fn sleep_list_take_where() {
        let mut s = SleepList::new();
        for i in 0..5u64 {
            s.insert(i * 10, i);
        }
        assert_eq!(s.take_where(|i| *i >= 3), [3, 4]);
        assert_eq!(s.next_wake(), Some(0));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn wait_queue() {
        let mut w = WaitQueue::new();
        assert!(w.is_empty());
        w.add(3);
        w.add(1);
        w.add(3);
        w.add(2);
        assert_eq!(w.len(), 3);
        assert!(w.remove(1));
        assert!(!w.remove(1));
        assert_eq!(w.pop(), Some(3));
        w.add(4);
        assert_eq!(w.drain(), [2, 4]);
        assert_eq!(w.pop(), None);
    }
}
