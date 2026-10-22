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
use crate::vfs::Node;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use frame::mm::pmm;
use frame::mm::shared::SharedMemory;
use frame::mm::user::UserFlags;
use frame::sync::SpinLock;

/// Pages of a file mapped `MAP_SHARED` (P13 K6): one set of frames for
/// every process that maps the same range of the same file, written back
/// to the file on `msync`, `munmap`, `execve` and `exit`/`exit_group`
/// (not from a destructor: that may run where file I/O cannot wait).
pub struct FileShare {
    node: Node,
    offset: u64,
    memory: SharedMemory,
}

impl FileShare {
    /// Bytes mapped (P14 K10).
    pub fn len(&self) -> u64 {
        self.memory.size()
    }

    /// Writes the pages back to the file (not past its current end).
    pub fn write_back(&self) {
        let Ok(info) = self.node.volume.fs.get(self.node.id) else {
            return;
        };
        if info.size <= self.offset {
            return;
        }
        let len = (info.size - self.offset).min(self.memory.size()) as usize;
        let mut buf = alloc::vec![0u8; len];
        if self.memory.read(0, &mut buf).is_ok() {
            let _ = self.node.volume.fs.write(self.node.id, self.offset, &buf);
        }
    }
}

/// The shared file ranges mapped somewhere now.
static SHARES: SpinLock<Vec<Weak<FileShare>>> = SpinLock::new(Vec::new());

/// The frames for `pages` pages of `node` from `offset` (made and filled
/// from the file if no one maps that range now).
fn share_for(node: &Node, offset: u64, pages: usize) -> Result<Arc<FileShare>, i64> {
    {
        let mut shares = SHARES.lock();
        shares.retain(|w| w.strong_count() > 0);
        let found = shares.iter().filter_map(Weak::upgrade).find(|s| {
            s.offset == offset
                && s.memory.pages() == pages
                && s.node.id == node.id
                && Arc::ptr_eq(&s.node.volume, &node.volume)
        });
        if let Some(s) = found {
            return Ok(s);
        }
    }
    let memory = SharedMemory::new(pages).ok_or(ENOMEM)?;
    let mut buf = alloc::vec![0u8; 64 * 1024];
    let mut done = 0u64;
    while done < memory.size() {
        let want = ((memory.size() - done) as usize).min(buf.len());
        let n = node
            .volume
            .fs
            .read(node.id, offset + done, &mut buf[..want])
            .map_err(super::errno::from_error)?;
        if n == 0 {
            break;
        }
        memory.write(done, &buf[..n]).map_err(|_| ENOMEM)?;
        done += n as u64;
    }
    let share = Arc::new(FileShare {
        node: node.clone(),
        offset,
        memory,
    });
    SHARES.lock().push(Arc::downgrade(&share));
    Ok(share)
}

/// Writes every shared file mapping of `p` back (it is ending).
pub fn flush_shared(p: &Process) {
    let Some(linux) = p.linux() else {
        return;
    };
    let maps: Vec<Arc<FileShare>> = linux
        .shared_maps
        .lock()
        .iter()
        .map(|(_, f)| Arc::clone(f))
        .collect();
    for f in maps {
        f.write_back();
    }
}

/// `mmap` of a file with `MAP_SHARED` (P13 K6): the shared frames, mapped
/// at a place the kernel chooses.
fn mmap_shared(p: &Process, len: u64, prot: u64, fd: u64, off: u64) -> LinuxResult {
    let file = super::sys::file_of(p, fd)?;
    let super::state::OpenFile::File { node, state } = &*file else {
        return Err(EACCES);
    };
    if prot & PROT_WRITE != 0 && !state.lock().writable {
        return Err(EACCES);
    }
    let pages = len.div_ceil(4096) as usize;
    let share = share_for(node, off, pages)?;
    let size = share.memory.size();
    let start = p.maps().lock().alloc(size).ok_or(ENOMEM)?;
    let flags = UserFlags {
        writable: prot & PROT_WRITE != 0,
        executable: false,
    };
    if p.space().map_shared(start, &share.memory, flags).is_err() {
        p.maps().lock().free(start);
        return Err(ENOMEM);
    }
    if let Some(linux) = p.linux() {
        linux.shared_maps.lock().push((start, share));
    }
    Ok(start)
}

/// `msync(addr, len, flags)`: the shared file mappings in the range are
/// written back.
pub fn msync(p: &Process, addr: u64, len: u64) -> LinuxResult {
    if !addr.is_multiple_of(4096) {
        return Err(EINVAL);
    }
    let end = addr.saturating_add(len);
    let Some(linux) = p.linux() else {
        return Ok(0);
    };
    let maps: Vec<Arc<FileShare>> = linux
        .shared_maps
        .lock()
        .iter()
        .filter(|(s, f)| *s < end && addr < *s + f.memory.size())
        .map(|(_, f)| Arc::clone(f))
        .collect();
    for f in maps {
        f.write_back();
    }
    Ok(0)
}

/// Removes the shared file mapping at `addr`, if there is one (written
/// back first); true if it did.
fn munmap_shared(p: &Process, addr: u64) -> bool {
    let Some(linux) = p.linux() else {
        return false;
    };
    let share = {
        let mut maps = linux.shared_maps.lock();
        let Some(i) = maps.iter().position(|(s, _)| *s == addr) else {
            return false;
        };
        maps.swap_remove(i).1
    };
    share.write_back();
    let _ = p.space().unmap_shared(addr);
    p.maps().lock().free(addr);
    true
}

/// Largest `brk` area.
pub const BRK_MAX: u64 = 256 << 20;

const PROT_READ: u64 = 1;
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
        if !shared && flags & MAP_PRIVATE == 0 {
            return Err(EINVAL);
        }
    } else if shared || flags & MAP_PRIVATE == 0 {
        return Err(ENOSYS);
    }
    if !off.is_multiple_of(4096) {
        return Err(EINVAL);
    }
    // P13 K6: a shared file mapping shares its frames (and is written
    // back); not at a fixed place, not executable.
    if shared {
        if fixed || prot & PROT_EXEC != 0 {
            return Err(EINVAL);
        }
        return mmap_shared(p, len, prot, fd, off);
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
    // P14 K9: anonymous PROT_NONE only reserves the range (Go reserves
    // hundreds of MiB this way); its pages come with mprotect or a
    // MAP_FIXED mapping over it.
    if anonymous && prot & (PROT_READ | PROT_WRITE | PROT_EXEC) == 0 {
        return Ok(start);
    }
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
    if munmap_shared(p, addr) {
        return Ok(0);
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
    let accessible = prot & (PROT_READ | PROT_WRITE | PROT_EXEC) != 0;
    let mut page = addr;
    while page < end {
        // P14 K9: a page of a PROT_NONE reservation gets its (zeroed) frame
        // when it becomes accessible; other unmapped pages are left alone.
        if p.space().protect(page, flags).is_err()
            && accessible
            && (abi::layout::MAP_BASE..abi::layout::MAP_END).contains(&page)
            && p.maps().lock().containing(page).is_some()
        {
            map_pages(p, page, page + 4096, flags)?;
        }
        page += 4096;
    }
    Ok(0)
}
