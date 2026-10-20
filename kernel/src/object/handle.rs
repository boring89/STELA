// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A process's handle table: slots holding an object reference and the
//! rights the handle grants. Pure logic, generic over the object type so
//! that it is tested on the host.
//!
//! ```text
//!  Handle 0x0200_0003 ──► slot 3, generation 2
//!
//!  slots  [0] gen 1  Port   SEND|RECEIVE|...
//!         [1] gen 1  Port   SEND
//!         [2] gen 1  (empty, reserved)
//!         [3] gen 2  Memory READ|WRITE|MAP   ◄── a freed slot reused
//!  free   [..]  slots ready for reuse, last freed first
//! ```
//!
//! Freeing a slot bumps its generation (255 wraps to 1, never 0), so a
//! stale handle value is refused instead of reaching the slot's next
//! object.

use abi::{Error, Handle, Rights};
use alloc::vec::Vec;

/// Most handles one table holds.
pub const MAX_HANDLES: usize = 1024;

/// An object reference and the rights of one handle to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry<T> {
    /// The object.
    pub object: T,
    /// What the handle allows.
    pub rights: Rights,
}

#[derive(Debug)]
struct Slot<T> {
    generation: u8,
    entry: Option<Entry<T>>,
}

/// A handle table.
#[derive(Debug)]
pub struct HandleTable<T> {
    slots: Vec<Slot<T>>,
    /// Indices of empty slots that may be reused.
    free: Vec<u32>,
}

impl<T> HandleTable<T> {
    /// An empty table.
    pub const fn new() -> HandleTable<T> {
        HandleTable {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// A table whose first slots hold `initial`, in order (handles
    /// `Handle::new(i, 1)`). Empty initial slots stay reserved: they are
    /// never handed out, so a bootstrap handle value never names something
    /// else.
    pub fn with_initial(initial: impl IntoIterator<Item = Option<Entry<T>>>) -> HandleTable<T> {
        let slots = initial
            .into_iter()
            .map(|entry| Slot {
                generation: 1,
                entry,
            })
            .collect();
        HandleTable {
            slots,
            free: Vec::new(),
        }
    }

    /// Number of handles in the table.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.entry.is_some()).count()
    }

    /// Whether the table holds no handle.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many more handles fit.
    pub fn room(&self) -> usize {
        self.free.len() + (MAX_HANDLES - self.slots.len().min(MAX_HANDLES))
    }

    /// Whether `n` more handles fit.
    pub fn has_room(&self, n: usize) -> bool {
        self.room() >= n
    }

    /// Adds an entry and returns its handle.
    pub fn insert(&mut self, entry: Entry<T>) -> Result<Handle, Error> {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.entry = Some(entry);
            return Ok(Handle::new(index, slot.generation));
        }
        if self.slots.len() >= MAX_HANDLES {
            return Err(Error::OutOfHandles);
        }
        let index = self.slots.len() as u32;
        self.slots.push(Slot {
            generation: 1,
            entry: Some(entry),
        });
        Ok(Handle::new(index, 1))
    }

    fn slot(&self, handle: Handle) -> Option<&Slot<T>> {
        let slot = self.slots.get(handle.index() as usize)?;
        (slot.generation == handle.generation() && slot.entry.is_some()).then_some(slot)
    }

    /// The entry of `handle`.
    pub fn get(&self, handle: Handle) -> Result<&Entry<T>, Error> {
        self.slot(handle)
            .and_then(|s| s.entry.as_ref())
            .ok_or(Error::BadHandle)
    }

    /// The entry of `handle`, which must have every right in `needed`.
    pub fn get_with(&self, handle: Handle, needed: Rights) -> Result<&Entry<T>, Error> {
        let entry = self.get(handle)?;
        if entry.rights.contains(needed) {
            Ok(entry)
        } else {
            Err(Error::AccessDenied)
        }
    }

    /// Removes `handle` and returns its entry.
    pub fn remove(&mut self, handle: Handle) -> Result<Entry<T>, Error> {
        self.slot(handle).ok_or(Error::BadHandle)?;
        let index = handle.index();
        let slot = &mut self.slots[index as usize];
        let entry = slot.entry.take().ok_or(Error::BadHandle)?;
        slot.generation = match slot.generation {
            u8::MAX => 1,
            g => g + 1,
        };
        self.free.push(index);
        Ok(entry)
    }

    /// Removes every entry (the process is ending) and returns them.
    pub fn drain(&mut self) -> Vec<Entry<T>> {
        let entries = self
            .slots
            .iter_mut()
            .filter_map(|s| s.entry.take())
            .collect();
        self.slots.clear();
        self.free.clear();
        entries
    }

    /// Every entry, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = &Entry<T>> {
        self.slots.iter().filter_map(|s| s.entry.as_ref())
    }
}

impl<T: Clone> HandleTable<T> {
    /// `HandleDuplicate`: a new handle to the same object with `rights`,
    /// which must be a subset of the original's; the original needs
    /// `DUPLICATE`.
    pub fn duplicate(&mut self, handle: Handle, rights: Rights) -> Result<Handle, Error> {
        let entry = self.get_with(handle, Rights::DUPLICATE)?;
        if !entry.rights.contains(rights) {
            return Err(Error::AccessDenied);
        }
        let object = entry.object.clone();
        self.insert(Entry { object, rights })
    }
}

impl<T> Default for HandleTable<T> {
    fn default() -> HandleTable<T> {
        HandleTable::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(object: u32, rights: Rights) -> Entry<u32> {
        Entry { object, rights }
    }

    #[test]
    fn insert_get_remove() {
        let mut t = HandleTable::new();
        let a = t.insert(entry(10, Rights::SEND)).unwrap();
        let b = t.insert(entry(20, Rights::ALL)).unwrap();
        assert_eq!(a, Handle::new(0, 1));
        assert_eq!(b, Handle::new(1, 1));
        assert_eq!(t.get(a).unwrap().object, 10);
        assert_eq!(t.len(), 2);
        assert_eq!(t.remove(a).unwrap().object, 10);
        // The old value is dead; the slot comes back with a new generation.
        assert_eq!(t.get(a), Err(Error::BadHandle));
        assert_eq!(t.remove(a), Err(Error::BadHandle));
        let c = t.insert(entry(30, Rights::NONE)).unwrap();
        assert_eq!(c, Handle::new(0, 2));
        assert_eq!(t.get(a), Err(Error::BadHandle));
        assert_eq!(t.get(c).unwrap().object, 30);
        assert_eq!(t.get(Handle::NONE), Err(Error::BadHandle));
        assert_eq!(t.get(Handle::new(99, 1)), Err(Error::BadHandle));
    }

    #[test]
    fn generation_wraps_to_one() {
        let mut t = HandleTable::new();
        let mut h = t.insert(entry(0, Rights::NONE)).unwrap();
        for _ in 0..300 {
            t.remove(h).unwrap();
            h = t.insert(entry(0, Rights::NONE)).unwrap();
            assert_ne!(h.generation(), 0);
            assert_eq!(h.index(), 0);
        }
    }

    #[test]
    fn rights_are_checked() {
        let mut t = HandleTable::new();
        let h = t
            .insert(entry(1, Rights::SEND | Rights::DUPLICATE))
            .unwrap();
        assert!(t.get_with(h, Rights::SEND).is_ok());
        assert_eq!(t.get_with(h, Rights::RECEIVE), Err(Error::AccessDenied));
        // Duplicate: only fewer rights.
        let d = t.duplicate(h, Rights::SEND).unwrap();
        assert_eq!(t.get(d).unwrap().rights, Rights::SEND);
        assert_eq!(t.get(d).unwrap().object, 1);
        assert_eq!(
            t.duplicate(h, Rights::SEND | Rights::RECEIVE),
            Err(Error::AccessDenied)
        );
        // Without DUPLICATE, no duplicates at all.
        assert_eq!(t.duplicate(d, Rights::SEND), Err(Error::AccessDenied));
    }

    #[test]
    fn table_limit() {
        let mut t = HandleTable::new();
        for i in 0..MAX_HANDLES {
            t.insert(entry(i as u32, Rights::NONE)).unwrap();
        }
        assert!(!t.has_room(1));
        assert_eq!(t.insert(entry(0, Rights::NONE)), Err(Error::OutOfHandles));
        t.remove(Handle::new(5, 1)).unwrap();
        assert!(t.has_room(1));
        assert!(!t.has_room(2));
        assert_eq!(t.insert(entry(0, Rights::NONE)).unwrap(), Handle::new(5, 2));
    }

    #[test]
    fn initial_slots() {
        let mut t = HandleTable::with_initial([
            Some(entry(1, Rights::ALL)),
            None,
            Some(entry(3, Rights::SEND)),
        ]);
        assert_eq!(t.get(abi::bootstrap::MAIN_PORT).unwrap().object, 1);
        assert_eq!(t.get(Handle::new(1, 1)), Err(Error::BadHandle));
        assert_eq!(t.get(Handle::new(2, 1)).unwrap().object, 3);
        // The empty slot is not reused.
        assert_eq!(t.insert(entry(4, Rights::NONE)).unwrap(), Handle::new(3, 1));
        assert_eq!(t.drain().len(), 3);
        assert!(t.is_empty());
    }
}
