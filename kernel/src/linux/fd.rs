// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! A Linux process's file descriptor table: small integers naming open
//! files. Pure logic over any entry type, tested on the host.
//!
//! ```text
//!  fd  0      1      2      3       4 ...
//!      [tty]  [tty]  [tty]  [file]  (free)      up to 256 entries
//! ```
//!
//! A new descriptor is the lowest free number (POSIX). `dup` shares the
//! entry (the caller's `T` is usually an `Arc`, so both descriptors see the
//! same offset, as in Linux); `FD_CLOEXEC` is per descriptor.

use alloc::vec::Vec;

/// Most descriptors a process may have open.
pub const FD_MAX: usize = 256;

/// One descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot<T> {
    /// The open file.
    pub file: T,
    /// `FD_CLOEXEC` (kept for `fcntl`; STELA has no `execve`).
    pub cloexec: bool,
}

/// Why a descriptor operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FdError {
    /// Not an open descriptor (EBADF).
    BadFd,
    /// The table is full (EMFILE).
    Full,
}

/// The table.
#[derive(Clone, Debug)]
pub struct FdTable<T> {
    slots: Vec<Option<Slot<T>>>,
}

impl<T: Clone> FdTable<T> {
    /// An empty table.
    pub const fn new() -> FdTable<T> {
        FdTable { slots: Vec::new() }
    }

    /// Opens `file` at the lowest free descriptor at least `min`.
    pub fn insert_from(&mut self, min: usize, file: T, cloexec: bool) -> Result<usize, FdError> {
        let free = (min..FD_MAX).find(|&fd| self.slots.get(fd).is_none_or(Option::is_none));
        let fd = free.ok_or(FdError::Full)?;
        if self.slots.len() <= fd {
            self.slots.resize(fd + 1, None);
        }
        self.slots[fd] = Some(Slot { file, cloexec });
        Ok(fd)
    }

    /// Opens `file` at the lowest free descriptor.
    pub fn insert(&mut self, file: T) -> Result<usize, FdError> {
        self.insert_from(0, file, false)
    }

    /// The descriptor `fd`.
    pub fn get(&self, fd: u64) -> Result<&Slot<T>, FdError> {
        usize::try_from(fd)
            .ok()
            .and_then(|fd| self.slots.get(fd))
            .and_then(Option::as_ref)
            .ok_or(FdError::BadFd)
    }

    /// The descriptor `fd`, to change its flags.
    pub fn get_mut(&mut self, fd: u64) -> Result<&mut Slot<T>, FdError> {
        usize::try_from(fd)
            .ok()
            .and_then(|fd| self.slots.get_mut(fd))
            .and_then(Option::as_mut)
            .ok_or(FdError::BadFd)
    }

    /// Closes `fd` and returns what it held.
    pub fn remove(&mut self, fd: u64) -> Result<T, FdError> {
        let slot = usize::try_from(fd)
            .ok()
            .and_then(|fd| self.slots.get_mut(fd))
            .and_then(Option::take)
            .ok_or(FdError::BadFd)?;
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
        Ok(slot.file)
    }

    /// `dup(fd)`: the same file at the lowest free descriptor.
    pub fn dup(&mut self, fd: u64) -> Result<usize, FdError> {
        let file = self.get(fd)?.file.clone();
        self.insert(file)
    }

    /// `dup2(fd, to)`: the same file at `to`, closing what was there
    /// (returned, so the caller can drop it outside its locks). `fd ==
    /// to` only checks `fd`.
    pub fn dup2(&mut self, fd: u64, to: u64) -> Result<(usize, Option<T>), FdError> {
        let file = self.get(fd)?.file.clone();
        let to = usize::try_from(to)
            .ok()
            .filter(|&t| t < FD_MAX)
            .ok_or(FdError::BadFd)?;
        if fd == to as u64 {
            return Ok((to, None));
        }
        if self.slots.len() <= to {
            self.slots.resize(to + 1, None);
        }
        let old = self.slots[to].replace(Slot {
            file,
            cloexec: false,
        });
        Ok((to, old.map(|s| s.file)))
    }

    /// Number of open descriptors.
    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Closes everything (the process ends).
    pub fn drain(&mut self) -> Vec<T> {
        core::mem::take(&mut self.slots)
            .into_iter()
            .flatten()
            .map(|s| s.file)
            .collect()
    }

    /// Removes the descriptors marked close-on-exec (`execve`) and returns
    /// their files.
    pub fn take_cloexec(&mut self) -> Vec<T> {
        let mut out = Vec::new();
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|s| s.cloexec)
                && let Some(s) = slot.take()
            {
                out.push(s.file);
            }
        }
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
        out
    }
}

impl<T: Clone> Default for FdTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowest_free_descriptor() {
        let mut t = FdTable::new();
        assert_eq!(t.insert('a'), Ok(0));
        assert_eq!(t.insert('b'), Ok(1));
        assert_eq!(t.insert('c'), Ok(2));
        assert_eq!(t.remove(1), Ok('b'));
        assert_eq!(t.insert('d'), Ok(1));
        assert_eq!(t.insert_from(10, 'e', true), Ok(10));
        assert!(t.get(10).unwrap().cloexec);
        assert_eq!(t.insert('f'), Ok(3));
        assert_eq!(t.count(), 5);
    }

    #[test]
    fn bad_descriptors() {
        let mut t: FdTable<u8> = FdTable::new();
        assert_eq!(t.get(0).err(), Some(FdError::BadFd));
        assert_eq!(t.remove(3).err(), Some(FdError::BadFd));
        assert_eq!(t.get(u64::MAX).err(), Some(FdError::BadFd));
        assert_eq!(t.dup(0).err(), Some(FdError::BadFd));
    }

    #[test]
    fn dup_and_dup2() {
        let mut t = FdTable::new();
        t.insert(7).unwrap();
        assert_eq!(t.dup(0), Ok(1));
        assert_eq!(t.get(1).unwrap().file, 7);
        t.insert(8).unwrap();
        assert_eq!(t.dup2(2, 0), Ok((0, Some(7))));
        assert_eq!(t.get(0).unwrap().file, 8);
        assert_eq!(t.dup2(0, 0), Ok((0, None)));
        assert_eq!(t.dup2(0, 50), Ok((50, None)));
        assert_eq!(t.dup2(0, 256).err(), Some(FdError::BadFd));
        assert_eq!(t.dup2(9, 1).err(), Some(FdError::BadFd));
    }

    #[test]
    fn full_table_and_drain() {
        let mut t = FdTable::new();
        for i in 0..FD_MAX {
            assert_eq!(t.insert(i), Ok(i));
        }
        assert_eq!(t.insert(0).err(), Some(FdError::Full));
        assert_eq!(t.drain().len(), FD_MAX);
        assert_eq!(t.count(), 0);
        assert_eq!(t.insert(1), Ok(0));
    }

    #[test]
    fn cloexec_descriptors_go_on_exec() {
        let mut t: FdTable<u32> = FdTable::new();
        t.insert(1).unwrap();
        t.insert_from(0, 2, true).unwrap();
        t.insert(3).unwrap();
        t.insert_from(0, 4, true).unwrap();
        assert_eq!(t.take_cloexec(), vec![2, 4]);
        assert_eq!(t.count(), 2);
        assert!(t.get(1).is_err());
        assert_eq!(t.get(2).unwrap().file, 3);
        assert_eq!(t.insert(5).unwrap(), 1);
    }
}
