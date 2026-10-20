// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Starting user programs: an ELF executable from the initrd is copied
//! into a new process's address space, a stack is mapped below
//! `STACK_TOP`, and the first thread starts at the entry point.
//!
//! ```text
//!  0x0000_0000_0040_0000   the program's PT_LOAD segments (W^X)
//!        ...
//!  0x0000_7FFF_FFDF_F000   guard page (not mapped)
//!  0x0000_7FFF_FFE0_0000   stack, 1 MiB, mapped at start
//!  0x0000_7FFF_FFF0_0000   STACK_TOP (initial RSP = STACK_TOP - 8)
//! ```

use crate::initrd;
use crate::object::Entry;
use crate::task::{self, Process, ProcessId, SpawnError};
use abi::layout::{STACK_GUARD, STACK_SIZE, STACK_TOP};
use alloc::sync::Arc;
use elfload::Elf;
use frame::mm::paging::MapError;
use frame::mm::pmm;
use frame::mm::user::{AddressSpace, UserFlags};

/// Why a program could not be started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// No such file in the initrd.
    NotFound,
    /// Not a valid ELF executable for STELA.
    BadElf(elfload::Error),
    /// A segment is outside the program area (below the stack guard), or
    /// both writable and executable.
    BadSegment,
    /// Out of physical memory.
    OutOfMemory,
}

impl From<MapError> for LoadError {
    fn from(e: MapError) -> LoadError {
        match e {
            MapError::OutOfMemory => LoadError::OutOfMemory,
            _ => LoadError::BadSegment,
        }
    }
}

/// Maps fresh zeroed frames at `start..end` (page aligned).
fn map_zeroed(
    space: &AddressSpace,
    start: u64,
    end: u64,
    flags: UserFlags,
) -> Result<(), LoadError> {
    let mut page = start;
    while page < end {
        let frame = pmm::alloc_zeroed_frame().ok_or(LoadError::OutOfMemory)?;
        space.map(page, frame, flags)?;
        page += 4096;
    }
    Ok(())
}

/// Copies the segments of `elf` into `space` and maps the stack.
fn load(space: &AddressSpace, elf: &Elf) -> Result<(), LoadError> {
    for segment in elf.segments() {
        let flags = UserFlags {
            writable: segment.flags.writable(),
            executable: segment.flags.executable(),
        };
        if flags.writable && flags.executable {
            return Err(LoadError::BadSegment);
        }
        let end = segment.vaddr_end().next_multiple_of(4096);
        if segment.vaddr == 0 || end > STACK_GUARD {
            return Err(LoadError::BadSegment);
        }
        // Zeroed frames: .bss (mem_size beyond file_size) needs nothing more.
        map_zeroed(space, segment.vaddr, end, flags)?;
        space
            .load(segment.vaddr, elf.segment_data(&segment))
            .map_err(|_| LoadError::BadSegment)?;
    }
    let stack = UserFlags {
        writable: true,
        executable: false,
    };
    map_zeroed(space, STACK_TOP - STACK_SIZE, STACK_TOP, stack)
}

/// Starts the ELF executable `image` as a new process named `name`.
pub fn spawn(name: &str, image: &[u8]) -> Result<ProcessId, LoadError> {
    spawn_process(name, image, None).map(|p| p.id())
}

/// Starts the ELF executable `image` as a new process named `name`, with
/// `bootstrap` as its handle `[2]`, and returns the process.
pub fn spawn_process(
    name: &str,
    image: &[u8],
    bootstrap: Option<Entry>,
) -> Result<Arc<Process>, LoadError> {
    let elf = Elf::parse(image).map_err(LoadError::BadElf)?;
    let process = task::process::create_with(name, bootstrap)?;
    // On failure the process has no thread; dropping it frees what was
    // mapped and ends it with code -1.
    load(process.space(), &elf)?;
    match task::spawn_user(&process, elf.entry(), STACK_TOP - 8, 0) {
        Ok(_) => Ok(process),
        Err(SpawnError::OutOfMemory | SpawnError::Exiting) => Err(LoadError::OutOfMemory),
    }
}

/// Starts the program `name` from the initrd.
pub fn spawn_from_initrd(name: &str) -> Result<ProcessId, LoadError> {
    spawn_initrd_process(name, None).map(|p| p.id())
}

/// Starts the program `name` from the initrd with `bootstrap` as its
/// handle `[2]`, and returns the process.
pub fn spawn_initrd_process(
    name: &str,
    bootstrap: Option<Entry>,
) -> Result<Arc<Process>, LoadError> {
    let image = initrd::find(frame::mm::initrd(), name).ok_or(LoadError::NotFound)?;
    spawn_process(name, image, bootstrap)
}
