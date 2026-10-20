// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Memory for Linux programs: the `brk` area after the program image, and
//! anonymous private `mmap`s in the process's map area (`MAP_BASE` up,
//! shared with `MemoryMap`). Pages are mapped zeroed at once (no demand
//! paging); executable mappings are refused (W^X).

use super::errno::{EACCES, EINVAL, ENOMEM, ENOSYS};
use super::sys::LinuxResult;
use crate::task::Process;
use frame::mm::pmm;
use frame::mm::user::UserFlags;

/// Largest `brk` area.
pub const BRK_MAX: u64 = 256 << 20;

const PROT_WRITE: u64 = 2;
const PROT_EXEC: u64 = 4;
const MAP_PRIVATE: u64 = 0x02;
const MAP_FIXED: u64 = 0x10;
const MAP_ANONYMOUS: u64 = 0x20;

const RW: UserFlags = UserFlags {
    writable: true,
    executable: false,
};

/// Maps zeroed pages at `start..end` (page aligned); on failure unmaps
/// what it mapped.
fn map_pages(p: &Process, start: u64, end: u64, flags: UserFlags) -> Result<(), i64> {
    let mut page = start;
    while page < end {
        let Some(frame) = pmm::alloc_zeroed_frame() else {
            unmap_pages(p, start, page);
            return Err(ENOMEM);
        };
        if p.space().map(page, frame, flags).is_err() {
            unmap_pages(p, start, page);
            return Err(ENOMEM);
        }
        page += 4096;
    }
    Ok(())
}

/// Unmaps (and frees) the pages mapped at `start..end`.
fn unmap_pages(p: &Process, start: u64, end: u64) {
    let mut page = start;
    while page < end {
        if let Ok(frame) = p.space().unmap(page) {
            pmm::free_frame(frame);
        }
        page += 4096;
    }
}

/// `brk(addr)`: moves the break; returns the (new or unchanged) break.
pub fn brk(p: &Process, addr: u64) -> u64 {
    let Some(linux) = p.linux() else {
        return 0;
    };
    let mut m = linux.memory.lock();
    if addr < m.start || addr > m.start + BRK_MAX {
        return m.current;
    }
    let end = addr.next_multiple_of(4096);
    if end > m.mapped {
        if map_pages(p, m.mapped, end, RW).is_err() {
            return m.current;
        }
    } else {
        unmap_pages(p, end, m.mapped);
    }
    m.mapped = end;
    m.current = addr;
    addr
}

/// `mmap(addr, len, prot, flags, fd, offset)`: anonymous private memory
/// only, placed by the kernel.
pub fn mmap(
    p: &Process,
    _addr: u64,
    len: u64,
    prot: u64,
    flags: u64,
    _fd: u64,
    _off: u64,
) -> LinuxResult {
    if len == 0 {
        return Err(EINVAL);
    }
    if flags & MAP_ANONYMOUS == 0 || flags & MAP_PRIVATE == 0 || flags & MAP_FIXED != 0 {
        return Err(ENOSYS);
    }
    if prot & PROT_EXEC != 0 {
        return Err(EACCES);
    }
    let size = len.checked_next_multiple_of(4096).ok_or(ENOMEM)?;
    let start = p.maps().lock().alloc(size).ok_or(ENOMEM)?;
    let flags = UserFlags {
        writable: prot & PROT_WRITE != 0,
        executable: false,
    };
    // PROT_NONE is mapped read-only (no pages without access in STELA).
    if let Err(e) = map_pages(p, start, start + size, flags) {
        p.maps().lock().free(start);
        return Err(e);
    }
    Ok(start)
}

/// `munmap(addr, len)`: frees the pages; a whole mapping also gives its
/// address range back.
pub fn munmap(p: &Process, addr: u64, len: u64) -> LinuxResult {
    if !addr.is_multiple_of(4096) || len == 0 {
        return Err(EINVAL);
    }
    let end = addr
        .checked_add(len.checked_next_multiple_of(4096).ok_or(EINVAL)?)
        .ok_or(EINVAL)?;
    if end > abi::layout::MAP_END || addr < abi::layout::MAP_BASE {
        // Not in the map area (the program, its stack or the brk area).
        return Err(EINVAL);
    }
    unmap_pages(p, addr, end);
    let mut maps = p.maps().lock();
    if maps.size_of(addr).is_some_and(|size| addr + size <= end) {
        maps.free(addr);
    }
    Ok(0)
}

/// `mprotect(addr, len, prot)`: changes write permission of mapped pages;
/// adding execute is refused.
pub fn mprotect(p: &Process, addr: u64, len: u64, prot: u64) -> LinuxResult {
    if !addr.is_multiple_of(4096) || prot & PROT_EXEC != 0 {
        return Err(EINVAL);
    }
    let end = addr
        .checked_add(len.checked_next_multiple_of(4096).ok_or(EINVAL)?)
        .ok_or(EINVAL)?;
    if end > abi::layout::USER_END {
        return Err(ENOMEM);
    }
    let flags = UserFlags {
        writable: prot & PROT_WRITE != 0,
        executable: false,
    };
    let mut page = addr;
    while page < end {
        // Unmapped pages are left alone.
        let _ = p.space().protect(page, flags);
        page += 4096;
    }
    Ok(0)
}
