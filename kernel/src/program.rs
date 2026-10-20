// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Starting user programs: an ELF executable (read from a file through the
//! VFS; the initrd is the `Initrd` volume) is copied into a new process's
//! address space, a stack is mapped below `STACK_TOP`, its arguments are
//! queued on its main port, and the first thread starts at the entry
//! point.
//!
//! ```text
//!  0x0000_0000_0040_0000   the program's PT_LOAD segments (W^X)
//!        ...
//!  0x0000_7FFF_FFDF_F000   guard page (not mapped)
//!  0x0000_7FFF_FFE0_0000   stack, 1 MiB, mapped at start
//!  0x0000_7FFF_FFF0_0000   STACK_TOP (initial RSP = STACK_TOP - 8)
//! ```

use crate::object::Entry;
use crate::object::port::QueuedMessage;
use crate::task::{self, Process, ProcessId, SpawnError};
use crate::vfs;
use abi::layout::{STACK_GUARD, STACK_SIZE, STACK_TOP};
use abi::{Message, tag};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use elfload::Elf;
use frame::mm::paging::MapError;
use frame::mm::pmm;
use frame::mm::user::{AddressSpace, UserFlags};

/// Why a program could not be started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// No such volume or file (or not a file).
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

/// Starts the ELF executable `image` as a new process named `name`, with
/// the standard bootstrap handles.
pub fn spawn(name: &str, image: &[u8]) -> Result<ProcessId, LoadError> {
    spawn_process(name, image, task::process::standard_initial(None), None).map(|p| p.id())
}

/// The arguments message of a program started with just its name.
pub fn name_args(name: &str) -> QueuedMessage {
    QueuedMessage::plain(Message::with_data(tag::BOOTSTRAP_ARGS, name.as_bytes()))
}

/// Starts the ELF executable `image` as a new process named `name`, with
/// `initial` as its handles `[1]`..`[3]` and `args` (default: just the
/// name) queued on its main port as its `BOOTSTRAP_ARGS` message.
pub fn spawn_process(
    name: &str,
    image: &[u8],
    initial: task::process::Initial,
    args: Option<QueuedMessage>,
) -> Result<Arc<Process>, LoadError> {
    let elf = Elf::parse(image).map_err(LoadError::BadElf)?;
    let process = task::process::create_with(name, initial)?;
    // On failure the process has no thread; dropping it frees what was
    // mapped and ends it with code -1.
    load(process.space(), &elf)?;
    let args = args.unwrap_or_else(|| name_args(name));
    if process.main_port().send(args).is_err() {
        return Err(LoadError::OutOfMemory);
    }
    match task::spawn_user(&process, elf.entry(), STACK_TOP - 8, 0) {
        Ok(_) => Ok(process),
        Err(SpawnError::OutOfMemory | SpawnError::Exiting) => Err(LoadError::OutOfMemory),
    }
}

/// Largest program file `read_program` takes.
pub const PROGRAM_MAX: u64 = 32 << 20;

/// The contents and name of file `id` on `volume` (a program to start).
pub fn read_program(volume: &vfs::Volume, id: u64) -> Result<(String, Vec<u8>), LoadError> {
    let info = volume.fs.get(id).map_err(|_| LoadError::NotFound)?;
    if info.is_dir || info.size > PROGRAM_MAX {
        return Err(LoadError::NotFound);
    }
    let mut image = vec![0u8; info.size as usize];
    let n = volume
        .fs
        .read(id, 0, &mut image)
        .map_err(|_| LoadError::NotFound)?;
    image.truncate(n);
    Ok((info.name, image))
}

/// Starts the program at `path` (relative to the root) on the volume
/// called `volume`, with the standard handles and `bootstrap` as `[2]`.
/// Programs are always read through the VFS (the initrd is the `Initrd`
/// volume).
pub fn spawn_path(
    volume: &str,
    path: &str,
    bootstrap: Option<Entry>,
) -> Result<Arc<Process>, LoadError> {
    let volume = vfs::find(volume).ok_or(LoadError::NotFound)?;
    let mut id = volume.fs.root();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        id = volume
            .fs
            .lookup(id, part)
            .map_err(|_| LoadError::NotFound)?
            .id;
    }
    let (name, image) = read_program(&volume, id)?;
    spawn_process(
        &name,
        &image,
        task::process::standard_initial(bootstrap),
        None,
    )
}
