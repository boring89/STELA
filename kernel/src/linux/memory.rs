// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Memory for Linux programs: the `brk` area after the program image, and
//! anonymous private `mmap`s in the process's map area (`MAP_BASE` up,
//! shared with `MemoryMap`). Pages are mapped zeroed at once (no demand
//! paging). Executable mappings (P13 K5, `docs/linux.md` §16): only private
//! copies of files in a library folder or the program's own folder, never
//! writable at the same time (W^X); anonymous executable memory is refused.

use super::errno::{EACCES, EINVAL, ENOMEM, ENOSYS};
use super::sys::LinuxResult;
use crate::task::Process;
use frame::mm::pmm;
use frame::mm::user::UserFlags;

/// Largest `brk` area.
pub const BRK_MAX: u64 = 256 << 20;

const PROT_WRITE: u64 = 2;
const PROT_EXEC: u64 = 4;
const MAP_SHARED: u64 = 0x01;
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

/// A new program image (start or `execve`, P13 K5): the `brk` area
/// starts after it; a PIE (loaded at `program::PIE_BASE`, inside the map
/// area) keeps its pages and its `brk` area out of `mmap`'s way.
pub fn set_image(p: &Process, image: &crate::program::LinuxImage) {
    let start = image.image_end.next_multiple_of(4096);
    if let Some(linux) = p.linux() {
        let mut m = linux.memory.lock();
        m.start = start;
        m.current = start;
        m.mapped = start;
        m.exec.clear();
    }
    let low = crate::program::PIE_BASE;
    if (abi::layout::MAP_BASE..abi::layout::MAP_END).contains(&start) && start > low {
        let _ = p.maps().lock().reserve(low, start + BRK_MAX - low);
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

/// `mmap(addr, len, prot, flags, fd, offset)`: anonymous private memory,
/// or a private copy of a file (P12: `MAP_PRIVATE`, and `MAP_SHARED`
/// without `PROT_WRITE`, which no one can tell apart from a copy), placed
/// by the kernel, or at `addr` with `MAP_FIXED` (P13 K5: over part of an
/// earlier mapping, or in a free part of the map area).
pub fn mmap(
    p: &Process,
    addr: u64,
    len: u64,
    prot: u64,
    flags: u64,
    fd: u64,
    off: u64,
) -> LinuxResult {
    if len == 0 {
        return Err(EINVAL);
    }
    let fixed = flags & MAP_FIXED != 0;
    let shared = flags & MAP_SHARED != 0;
    if flags & MAP_ANONYMOUS == 0 {
        if shared && prot & PROT_WRITE != 0 {
            return Err(ENOSYS);
        }
        if !shared && flags & MAP_PRIVATE == 0 {
            return Err(EINVAL);
        }
    } else if shared || flags & MAP_PRIVATE == 0 {
        return Err(ENOSYS);
    }
    if !off.is_multiple_of(4096) {
        return Err(EINVAL);
    }
    let anonymous = flags & MAP_ANONYMOUS != 0;
    let exec = prot & PROT_EXEC != 0;
    if exec {
        // W^X, and only files from the library folders (P13 K5).
        if anonymous || shared || prot & PROT_WRITE != 0 {
            return Err(EACCES);
        }
        let file = super::sys::file_of(p, fd)?;
        let super::state::OpenFile::File { node, .. } = &*file else {
            return Err(EACCES);
        };
        if !super::files::may_execute(p, node) {
            return Err(EACCES);
        }
    }
    let size = len.checked_next_multiple_of(4096).ok_or(ENOMEM)?;
    // Where: chosen, or at addr (MAP_FIXED): inside an earlier mapping its
    // pages there are replaced; elsewhere the range is reserved.
    let (start, new_range) = if fixed {
        if !addr.is_multiple_of(4096) {
            return Err(EINVAL);
        }
        let end = addr.checked_add(size).ok_or(ENOMEM)?;
        let mut maps = p.maps().lock();
        match maps.containing(addr) {
            Some((s, l)) if end <= s + l => {
                drop(maps);
                unmap_pages(p, addr, end);
                (addr, false)
            }
            _ if maps.reserve(addr, size) => (addr, true),
            _ => return Err(ENOMEM),
        }
    } else {
        (p.maps().lock().alloc(size).ok_or(ENOMEM)?, true)
    };
    let give_back = |p: &Process| {
        if new_range {
            p.maps().lock().free(start);
        }
    };
    let flags = UserFlags {
        writable: prot & PROT_WRITE != 0,
        executable: exec,
    };
    // PROT_NONE is mapped read-only (no pages without access in STELA).
    // A file's bytes are copied in while the pages are still writable.
    let writable = UserFlags {
        writable: true,
        executable: false,
    };
    let first = if anonymous { flags } else { writable };
    if let Err(e) = map_pages(p, start, start + size, first) {
        give_back(p);
        return Err(e);
    }
    if !anonymous {
        if let Err(e) = copy_file(p, fd, off, start, len) {
            unmap_pages(p, start, start + size);
            give_back(p);
            return Err(e);
        }
        if exec && let Some(linux) = p.linux() {
            // mprotect may make it executable again later (RELRO, ...).
            let range = match p.maps().lock().containing(start) {
                Some((s, l)) => (s, s + l),
                None => (start, start + size),
            };
            let mut m = linux.memory.lock();
            if !m.exec.contains(&range) {
                m.exec.push(range);
            }
        }
        if !flags.writable {
            let mut page = start;
            while page < start + size {
                let _ = p.space().protect(page, flags);
                page += 4096;
            }
        }
    }
    Ok(start)
}

/// Copies `len` bytes of file `fd` from `off` to `at` (past the end of
/// the file the pages stay zero).
fn copy_file(p: &Process, fd: u64, off: u64, at: u64, len: u64) -> Result<(), i64> {
    let file = super::sys::file_of(p, fd)?;
    let super::state::OpenFile::File { node, .. } = &*file else {
        return Err(EACCES);
    };
    let mut done = 0u64;
    let mut buf = alloc::vec![0u8; 64 * 1024];
    while done < len {
        let want = ((len - done) as usize).min(buf.len());
        let n = node
            .volume
            .fs
            .read(node.id, off + done, &mut buf[..want])
            .map_err(super::errno::from_error)?;
        if n == 0 {
            break;
        }
        p.space()
            .write(at + done, &buf[..n])
            .map_err(|_| super::errno::EFAULT)?;
        done += n as u64;
    }
    Ok(())
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
        if let Some(linux) = p.linux() {
            linux.memory.lock().exec.retain(|&(s, _)| s != addr);
        }
    }
    Ok(0)
}

/// `mprotect(addr, len, prot)`: changes the permissions of mapped pages;
/// execute only inside a mapping of a library file, and never with write
/// (P13 K5).
pub fn mprotect(p: &Process, addr: u64, len: u64, prot: u64) -> LinuxResult {
    if !addr.is_multiple_of(4096) {
        return Err(EINVAL);
    }
    let end = addr
        .checked_add(len.checked_next_multiple_of(4096).ok_or(EINVAL)?)
        .ok_or(EINVAL)?;
    if end > abi::layout::USER_END {
        return Err(ENOMEM);
    }
    let exec = prot & PROT_EXEC != 0;
    if exec {
        let allowed = prot & PROT_WRITE == 0
            && p.linux().is_some_and(|l| {
                l.memory
                    .lock()
                    .exec
                    .iter()
                    .any(|&(s, e)| s <= addr && end <= e)
            });
        if !allowed {
            return Err(EACCES);
        }
    }
    let flags = UserFlags {
        writable: prot & PROT_WRITE != 0,
        executable: exec,
    };
    let mut page = addr;
    while page < end {
        // Unmapped pages are left alone.
        let _ = p.space().protect(page, flags);
        page += 4096;
    }
    Ok(0)
}
