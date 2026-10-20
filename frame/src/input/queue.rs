// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A fixed-size ring buffer of events. When it is full, the oldest event is
//! dropped (and counted) to make room: input that nobody read for a while
//! is the least valuable. Pure logic, tested on the host.

/// Up to `N` events of type `T`, oldest first.
#[derive(Debug)]
pub struct EventQueue<T: Copy, const N: usize> {
    slots: [Option<T>; N],
    /// Index of the oldest event.
    head: usize,
    len: usize,
    dropped: u64,
}

impl<T: Copy, const N: usize> EventQueue<T, N> {
    /// An empty queue.
    pub const fn new() -> EventQueue<T, N> {
        EventQueue {
            slots: [None; N],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    /// Appends `event`, dropping the oldest one if the queue is full.
    pub fn push(&mut self, event: T) {
        if N == 0 {
            self.dropped += 1;
            return;
        }
        if self.len == N {
            self.head = (self.head + 1) % N;
            self.len -= 1;
            self.dropped += 1;
        }
        self.slots[(self.head + self.len) % N] = Some(event);
        self.len += 1;
    }

    /// Removes and returns the oldest event.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let event = self.slots[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        event
    }

    /// Number of queued events.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no event is queued.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Events dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl<T: Copy, const N: usize> Default for EventQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_in_first_out() {
        let mut q: EventQueue<u32, 4> = EventQueue::new();
        assert_eq!(q.pop(), None);
        q.push(1);
        q.push(2);
        assert_eq!(q.len(), 2);
        assert_eq!(q.pop(), Some(1));
        q.push(3);
        q.push(4);
        q.push(5);
        assert_eq!(
            (q.pop(), q.pop(), q.pop(), q.pop()),
            (Some(2), Some(3), Some(4), Some(5))
        );
        assert!(q.is_empty());
        assert_eq!(q.dropped(), 0);
    }

    #[test]
    fn full_queue_drops_the_oldest() {
        let mut q: EventQueue<u32, 3> = EventQueue::new();
        for i in 0..10 {
            q.push(i);
        }
        assert_eq!(q.len(), 3);
        assert_eq!(q.dropped(), 7);
        assert_eq!(q.pop(), Some(7));
        assert_eq!(q.pop(), Some(8));
        assert_eq!(q.pop(), Some(9));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn matches_a_model_over_many_wraps() {
        let mut q: EventQueue<u64, 5> = EventQueue::new();
        let mut model = std::collections::VecDeque::new();
        let mut dropped = 0;
        for i in 0..1000u64 {
            // Push one or two, pop one: the queue fills up and wraps.
            for value in [i, i + 10_000].iter().take(1 + (i % 2) as usize) {
                q.push(*value);
                model.push_back(*value);
                if model.len() > 5 {
                    model.pop_front();
                    dropped += 1;
                }
            }
            assert_eq!(q.pop(), model.pop_front());
            assert_eq!(q.len(), model.len());
        }
        assert_eq!(q.dropped(), dropped);
        assert!(dropped > 0);
    }
}
