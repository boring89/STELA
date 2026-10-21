// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files and folders (`docs/vfs.md`): every call is relative to a folder
//! handle; `volume_open` gives the root of a volume by name.

use crate::sys::{syscall2, syscall3, syscall4, syscall6};
use abi::{
    DIR_ENTRY_SIZE, DirEntry, Error, Handle, STAT_SIZE, Stat, Syscall, VOLUME_INFO_SIZE, VolumeInfo,
};

pub use abi::open;

/// Opens (or with `open::CREATE` makes) the item at the relative `path`
/// under `dir`.
pub fn open(dir: Handle, path: &str, flags: u64) -> Result<Handle, Error> {
    syscall4(
        Syscall::DirOpen,
        dir.raw(),
        path.as_ptr() as u64,
        path.len() as u64,
        flags,
    )
    .map(Handle::from_raw)
}

/// The entry of folder `dir` after `cookie` (0: the first) and the
/// cookie of the next one (`None` past the last). Cookies are opaque
/// (eHFS: the last item's CNID).
pub fn read_dir(dir: Handle, cookie: u64) -> Result<Option<(DirEntry, u64)>, Error> {
    let mut buf = [0u8; DIR_ENTRY_SIZE];
    let next = syscall3(Syscall::DirRead, dir.raw(), cookie, buf.as_mut_ptr() as u64)?;
    if next == 0 {
        return Ok(None);
    }
    let mut e = DirEntry::new();
    e.id = u64::from_le_bytes(buf[0..8].try_into().unwrap_or_default());
    e.size = u64::from_le_bytes(buf[8..16].try_into().unwrap_or_default());
    e.mtime = u64::from_le_bytes(buf[16..24].try_into().unwrap_or_default());
    e.kind = u32::from_le_bytes(buf[24..28].try_into().unwrap_or_default());
    e.name_len = u32::from_le_bytes(buf[28..32].try_into().unwrap_or_default());
    e.name.copy_from_slice(&buf[32..]);
    Ok(Some((e, next)))
}

/// Every entry of folder `dir`.
pub fn entries(dir: Handle) -> Result<alloc::vec::Vec<DirEntry>, Error> {
    let mut out = alloc::vec::Vec::new();
    let mut cookie = 0;
    while let Some((e, next)) = read_dir(dir, cookie)? {
        out.push(e);
        cookie = next;
    }
    Ok(out)
}

/// Removes the file or empty folder `name` in `dir`.
pub fn remove(dir: Handle, name: &str) -> Result<(), Error> {
    syscall3(
        Syscall::DirRemove,
        dir.raw(),
        name.as_ptr() as u64,
        name.len() as u64,
    )
    .map(|_| ())
}

/// Moves `old` in `dir` to `new` in `new_dir` (same volume).
pub fn rename(dir: Handle, old: &str, new_dir: Handle, new: &str) -> Result<(), Error> {
    syscall6(
        Syscall::DirRename,
        [
            dir.raw(),
            old.as_ptr() as u64,
            old.len() as u64,
            new_dir.raw(),
            new.as_ptr() as u64,
            new.len() as u64,
        ],
    )
    .map(|_| ())
}

/// Reads at `offset` into `buf`; returns the bytes read (0 at the end).
pub fn read(file: Handle, buf: &mut [u8], offset: u64) -> Result<usize, Error> {
    syscall4(
        Syscall::FileRead,
        file.raw(),
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
        offset,
    )
    .map(|n| n as usize)
}

/// Writes `data` at `offset`.
pub fn write(file: Handle, data: &[u8], offset: u64) -> Result<usize, Error> {
    syscall4(
        Syscall::FileWrite,
        file.raw(),
        data.as_ptr() as u64,
        data.len() as u64,
        offset,
    )
    .map(|n| n as usize)
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut w = [0u8; 8];
    w.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(w)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut w = [0u8; 4];
    w.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(w)
}

/// Information about a file or folder handle.
pub fn stat(h: Handle) -> Result<Stat, Error> {
    let mut b = [0u8; STAT_SIZE];
    syscall2(Syscall::FileStat, h.raw(), b.as_mut_ptr() as u64)?;
    Ok(Stat {
        id: u64_at(&b, 0),
        size: u64_at(&b, 8),
        ctime: u64_at(&b, 16),
        mtime: u64_at(&b, 24),
        kind: u32_at(&b, 32),
        volume: u32_at(&b, 36),
        flags: u32_at(&b, 40),
        reserved: 0,
    })
}

/// Sets a file's size.
pub fn set_size(file: Handle, size: u64) -> Result<(), Error> {
    syscall2(Syscall::FileSetSize, file.raw(), size).map(|_| ())
}

/// The volume whose index (`VolumeInfo::index`, `Stat::volume`) is
/// `index`: volumes come and go (P11 K41), so the list's position is not
/// the index.
pub fn volume_by_index(index: u32) -> Option<VolumeInfo> {
    let mut i = 0;
    while let Ok(Some(v)) = volume_list(i) {
        if v.index == index {
            return Some(v);
        }
        i += 1;
    }
    None
}

/// Volume `index` (`None` past the last).
pub fn volume_list(index: u64) -> Result<Option<VolumeInfo>, Error> {
    let mut b = [0u8; VOLUME_INFO_SIZE];
    if syscall2(Syscall::VolumeList, index, b.as_mut_ptr() as u64)? == 0 {
        return Ok(None);
    }
    let mut v = VolumeInfo::new();
    v.index = u32_at(&b, 0);
    v.flags = u32_at(&b, 4);
    v.kind = u32_at(&b, 8);
    v.name_len = u32_at(&b, 12);
    v.total = u64_at(&b, 16);
    v.free = u64_at(&b, 24);
    v.name.copy_from_slice(&b[32..96]);
    Ok(Some(v))
}

/// The root folder of the volume called `name`.
pub fn volume_open(name: &str) -> Result<Handle, Error> {
    volume_open_inner(name)
}

/// The Finder information (32 bytes) of the file or folder `h`
/// (`NotSupported` on FAT and the initrd).
pub fn get_info(h: Handle) -> Result<[u8; abi::FINDER_INFO_SIZE], Error> {
    let mut info = [0u8; abi::FINDER_INFO_SIZE];
    crate::sys::syscall2(Syscall::FileGetInfo, h.raw(), info.as_mut_ptr() as u64)?;
    Ok(info)
}

/// The path of item `h` inside its volume (`/`, `/a/b`; `FilePath`).
pub fn path_of(h: Handle) -> Result<alloc::string::String, Error> {
    let mut buf = alloc::vec![0u8; 256];
    loop {
        let n = syscall4(
            Syscall::FilePath,
            h.raw(),
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0,
        )? as usize;
        if n <= buf.len() {
            buf.truncate(n);
            return alloc::string::String::from_utf8(buf).map_err(|_| Error::InvalidArgument);
        }
        buf.resize(n, 0);
    }
}

/// The value of extended attribute `name` of item `h` (`NotFound`
/// without one; P11).
pub fn get_attr(h: Handle, name: &str) -> Result<alloc::vec::Vec<u8>, Error> {
    let mut buf = alloc::vec![0u8; 256];
    loop {
        let n = syscall6(
            Syscall::FileGetAttr,
            [
                h.raw(),
                name.as_ptr() as u64,
                name.len() as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                0,
            ],
        )? as usize;
        if n <= buf.len() {
            buf.truncate(n);
            return Ok(buf);
        }
        buf.resize(n, 0);
    }
}

/// Sets extended attribute `name` of `h` (≤ `ATTR_VALUE_MAX` bytes; an
/// empty value removes it). Needs `WRITE` and `MANAGE`.
pub fn set_attr(h: Handle, name: &str, value: &[u8]) -> Result<(), Error> {
    syscall6(
        Syscall::FileSetAttr,
        [
            h.raw(),
            name.as_ptr() as u64,
            name.len() as u64,
            value.as_ptr() as u64,
            value.len() as u64,
            0,
        ],
    )
    .map(|_| ())
}

/// The names of `h`'s extended attributes.
pub fn list_attrs(h: Handle) -> Result<alloc::vec::Vec<alloc::string::String>, Error> {
    let mut buf = alloc::vec![0u8; 256];
    loop {
        let n = syscall3(
            Syscall::FileListAttr,
            h.raw(),
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )? as usize;
        if n <= buf.len() {
            return Ok(buf[..n]
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| alloc::string::String::from_utf8_lossy(s).into_owned())
                .collect());
        }
        buf.resize(n, 0);
    }
}

/// Stores the Finder information of `h` (which needs `WRITE` and
/// `MANAGE`).
pub fn set_info(h: Handle, info: &[u8; abi::FINDER_INFO_SIZE]) -> Result<(), Error> {
    crate::sys::syscall2(Syscall::FileSetInfo, h.raw(), info.as_ptr() as u64).map(|_| ())
}

fn volume_open_inner(name: &str) -> Result<Handle, Error> {
    syscall2(Syscall::VolumeOpen, name.as_ptr() as u64, name.len() as u64).map(Handle::from_raw)
}

/// The item with persistent id `id` on `dir`'s volume.
pub fn open_by_id(dir: Handle, id: u64) -> Result<Handle, Error> {
    syscall2(Syscall::VolumeOpenById, dir.raw(), id).map(Handle::from_raw)
}
