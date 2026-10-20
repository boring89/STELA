// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The kernel side of the VFS: the backend trait, volumes, nodes and the
//! eHFS and FAT backends.

use crate::block::Disk;
use crate::fs::cache::CachedDevice;
use crate::fs::fat::{Dir, FatEntry, FatError, FatVolume};
use abi::Error;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use block::Partition;
use drivers::sync::SleepMutex;
use frame::sync::SpinLock;

/// A file or folder as the backends describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    /// Persistent id on its volume.
    pub id: u64,
    /// A folder.
    pub is_dir: bool,
    /// Size in bytes (files).
    pub size: u64,
    /// Creation time, ns since 1970 (0 if unknown).
    pub ctime: u64,
    /// Modification time.
    pub mtime: u64,
    /// The name.
    pub name: String,
}

/// What a file system offers the VFS. Ids are persistent; the root has
/// [`FileSystem::root`]'s id.
pub trait FileSystem: Send + Sync {
    /// The root folder's id.
    fn root(&self) -> u64;
    /// The item `name` in folder `dir`.
    fn lookup(&self, dir: u64, name: &str) -> Result<NodeInfo, Error>;
    /// The item with id `id`.
    fn get(&self, id: u64) -> Result<NodeInfo, Error>;
    /// The items of folder `dir`.
    fn list(&self, dir: u64) -> Result<Vec<NodeInfo>, Error>;
    /// Reads at `offset` of file `id`; returns the bytes read.
    fn read(&self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error>;
    /// Writes at `offset` of file `id`.
    fn write(&self, id: u64, offset: u64, data: &[u8]) -> Result<(), Error>;
    /// Makes a file or folder.
    fn create(&self, dir: u64, name: &str, is_dir: bool) -> Result<NodeInfo, Error>;
    /// Removes a file or an empty folder.
    fn remove(&self, dir: u64, name: &str) -> Result<(), Error>;
    /// Moves and/or renames.
    fn rename(&self, dir: u64, name: &str, new_dir: u64, new_name: &str) -> Result<(), Error>;
    /// Sets a file's size.
    fn set_size(&self, id: u64, size: u64) -> Result<(), Error>;
    /// Whether writes are refused.
    fn read_only(&self) -> bool;
    /// (total, free) bytes.
    fn space(&self) -> (u64, u64);
    /// `abi::volume_kind`.
    fn kind(&self) -> u32;
    /// Makes everything durable and marks the volume clean (shutdown).
    fn unmount(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// A mounted volume.
pub struct Volume {
    /// Index in the volume list (stable while the system runs).
    pub index: u32,
    /// The name programs use (`Data`, `Boot`, ...).
    pub name: String,
    /// The backend.
    pub fs: Arc<dyn FileSystem>,
}

/// A folder or file handle's target.
#[derive(Clone)]
pub struct Node {
    /// Its volume.
    pub volume: Arc<Volume>,
    /// Its persistent id.
    pub id: u64,
}

static VOLUMES: SpinLock<Vec<Arc<Volume>>> = SpinLock::new(Vec::new());

/// Adds a volume under `name` (made unique: "Name 2", ...); returns it.
pub fn mount(name: &str, fs: Arc<dyn FileSystem>) -> Arc<Volume> {
    let mut volumes = VOLUMES.lock();
    let mut unique = String::from(name);
    let mut n = 2;
    while volumes.iter().any(|v| v.name.eq_ignore_ascii_case(&unique)) {
        unique = alloc::format!("{name} {n}");
        n += 1;
    }
    let volume = Arc::new(Volume {
        index: volumes.len() as u32,
        name: unique,
        fs,
    });
    volumes.push(Arc::clone(&volume));
    volume
}

/// The volumes, in mount order.
pub fn volumes() -> Vec<Arc<Volume>> {
    VOLUMES.lock().clone()
}

/// The volume called `name` (ASCII case ignored).
pub fn find(name: &str) -> Option<Arc<Volume>> {
    VOLUMES
        .lock()
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(name))
        .cloned()
}

/// Unmounts every volume (marks eHFS volumes clean). Errors are printed.
pub fn unmount_all() {
    for v in volumes() {
        if let Err(e) = v.fs.unmount() {
            frame::kprintln!("vfs: unmounting {}: {}", v.name, e.name());
        }
    }
}

// ---- eHFS ----

/// The device under an eHFS or FAT volume: a partition of a disk, with
/// the block cache in front.
pub type VolumeDevice = CachedDevice<Partition<Disk>>;

fn ehfs_error(e: ehfs::Error) -> Error {
    match e {
        ehfs::Error::NotFound => Error::NotFound,
        ehfs::Error::Exists => Error::Exists,
        ehfs::Error::NotDir => Error::NotDir,
        ehfs::Error::IsDir => Error::IsDir,
        ehfs::Error::NotEmpty => Error::NotEmpty,
        ehfs::Error::ReadOnly => Error::ReadOnly,
        ehfs::Error::NoSpace => Error::NoSpace,
        ehfs::Error::BadName | ehfs::Error::Invalid => Error::InvalidArgument,
        ehfs::Error::Io
        | ehfs::Error::Corrupt
        | ehfs::Error::TxnTooLarge
        | ehfs::Error::Unsupported => Error::IoError,
    }
}

fn ehfs_info(e: &ehfs::Entry) -> NodeInfo {
    NodeInfo {
        id: e.cnid,
        is_dir: e.is_dir,
        size: if e.is_dir { 0 } else { e.size() },
        ctime: e.times[0],
        mtime: e.times[1],
        name: String::from_utf8_lossy(&e.name).into(),
    }
}

/// An eHFS volume.
pub struct EhfsFs {
    volume: SleepMutex<ehfs::Volume<VolumeDevice>>,
    read_only: bool,
}

impl EhfsFs {
    /// Mounts the eHFS volume on `device` (replaying its journal).
    pub fn mount(device: VolumeDevice, read_only: bool) -> Result<(EhfsFs, String), ehfs::Error> {
        let v = ehfs::Volume::mount(device, read_only)?;
        let name = v.name();
        let read_only = v.read_only();
        Ok((
            EhfsFs {
                volume: SleepMutex::new(v),
                read_only,
            },
            name,
        ))
    }

    fn with<R>(
        &self,
        f: impl FnOnce(&mut ehfs::Volume<VolumeDevice>) -> Result<R, ehfs::Error>,
    ) -> Result<R, Error> {
        self.volume.with(f).map_err(ehfs_error)
    }
}

impl FileSystem for EhfsFs {
    fn root(&self) -> u64 {
        ehfs::ROOT
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<NodeInfo, Error> {
        self.with(|v| v.lookup(dir, name.as_bytes()))
            .map(|e| ehfs_info(&e))
    }

    fn get(&self, id: u64) -> Result<NodeInfo, Error> {
        self.with(|v| v.get(id)).map(|e| ehfs_info(&e))
    }

    fn list(&self, dir: u64) -> Result<Vec<NodeInfo>, Error> {
        self.with(|v| {
            if !v.get(dir)?.is_dir {
                return Err(ehfs::Error::NotDir);
            }
            v.list(dir)
        })
        .map(|l| l.iter().map(ehfs_info).collect())
    }

    fn read(&self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        self.with(|v| v.read(id, false, offset, buf))
    }

    fn write(&self, id: u64, offset: u64, data: &[u8]) -> Result<(), Error> {
        self.with(|v| v.write(id, false, offset, data, crate::fs::now()))
    }

    fn create(&self, dir: u64, name: &str, is_dir: bool) -> Result<NodeInfo, Error> {
        self.with(|v| v.create(dir, name.as_bytes(), is_dir, crate::fs::now()))
            .map(|e| ehfs_info(&e))
    }

    fn remove(&self, dir: u64, name: &str) -> Result<(), Error> {
        self.with(|v| v.remove(dir, name.as_bytes(), crate::fs::now()))
    }

    fn rename(&self, dir: u64, name: &str, new_dir: u64, new_name: &str) -> Result<(), Error> {
        self.with(|v| {
            v.rename(
                dir,
                name.as_bytes(),
                new_dir,
                new_name.as_bytes(),
                crate::fs::now(),
            )
        })
    }

    fn set_size(&self, id: u64, size: u64) -> Result<(), Error> {
        self.with(|v| v.set_length(id, false, size, crate::fs::now()))
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn space(&self) -> (u64, u64) {
        self.volume
            .with(|v| (v.block_count() * 4096, v.free_blocks() * 4096))
    }

    fn kind(&self) -> u32 {
        abi::volume_kind::EHFS
    }

    fn unmount(&self) -> Result<(), Error> {
        self.with(|v| v.unmount())
    }
}

// ---- FAT ----

fn fat_error(e: FatError) -> Error {
    match e {
        FatError::Device(_) | FatError::Corrupt | FatError::NotFat | FatError::Fat12 => {
            Error::IoError
        }
    }
}

/// Unix time of a FAT date and time (no time zone: taken as UTC).
fn fat_time(t: (u16, u8, u8, u8, u8, u8)) -> u64 {
    let (y, mo, d, h, mi, s) = t;
    // Days from 1970-01-01 to the date (proleptic Gregorian).
    let (y, mo) = (i64::from(y), i64::from(mo.clamp(1, 12)));
    let (yy, mm) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + i64::from(d.max(1)) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s);
    (secs.max(0) as u64) * 1_000_000_000
}

/// A FAT volume, read only. FAT has no persistent ids: entries get ids as
/// they are seen (stable while the system runs).
pub struct FatFs {
    fat: SleepMutex<FatVolume<VolumeDevice>>,
    /// id -> (entry, the folder it is in).
    ids: SpinLock<BTreeMap<u64, FatEntry>>,
    /// (folder id, upper-case name) -> id.
    names: SpinLock<BTreeMap<(u64, String), u64>>,
}

const FAT_ROOT: u64 = 1;

impl FatFs {
    /// Mounts the FAT volume on `device`.
    pub fn mount(device: VolumeDevice) -> Result<(FatFs, String), FatError> {
        let fat = FatVolume::mount(device)?;
        let label = String::from(fat.label());
        Ok((
            FatFs {
                fat: SleepMutex::new(fat),
                ids: SpinLock::new(BTreeMap::new()),
                names: SpinLock::new(BTreeMap::new()),
            },
            label,
        ))
    }

    fn id_for(&self, dir: u64, entry: &FatEntry) -> u64 {
        let key = (dir, entry.name.to_ascii_uppercase());
        let mut names = self.names.lock();
        if let Some(&id) = names.get(&key) {
            self.ids.lock().insert(id, entry.clone());
            return id;
        }
        let mut ids = self.ids.lock();
        let id = ids.keys().next_back().map_or(FAT_ROOT + 1, |k| k + 1);
        ids.insert(id, entry.clone());
        names.insert(key, id);
        id
    }

    fn info(&self, dir: u64, e: &FatEntry) -> NodeInfo {
        let t = fat_time(e.modified);
        NodeInfo {
            id: self.id_for(dir, e),
            is_dir: e.is_dir,
            size: u64::from(e.size),
            ctime: t,
            mtime: t,
            name: e.name.clone(),
        }
    }

    fn dir_of(&self, id: u64) -> Result<Dir, Error> {
        if id == FAT_ROOT {
            return Ok(self.fat.with(|f| f.root()));
        }
        let entry = self.ids.lock().get(&id).cloned().ok_or(Error::NotFound)?;
        if !entry.is_dir {
            return Err(Error::NotDir);
        }
        Ok(self.fat.with(|f| f.dir_of(&entry)))
    }
}

impl FileSystem for FatFs {
    fn root(&self) -> u64 {
        FAT_ROOT
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<NodeInfo, Error> {
        let d = self.dir_of(dir)?;
        let found = self
            .fat
            .with(|f| f.lookup(d, name))
            .map_err(fat_error)?
            .ok_or(Error::NotFound)?;
        Ok(self.info(dir, &found))
    }

    fn get(&self, id: u64) -> Result<NodeInfo, Error> {
        if id == FAT_ROOT {
            return Ok(NodeInfo {
                id,
                is_dir: true,
                size: 0,
                ctime: 0,
                mtime: 0,
                name: String::new(),
            });
        }
        let e = self.ids.lock().get(&id).cloned().ok_or(Error::NotFound)?;
        let t = fat_time(e.modified);
        Ok(NodeInfo {
            id,
            is_dir: e.is_dir,
            size: u64::from(e.size),
            ctime: t,
            mtime: t,
            name: e.name,
        })
    }

    fn list(&self, dir: u64) -> Result<Vec<NodeInfo>, Error> {
        let d = self.dir_of(dir)?;
        let entries = self.fat.with(|f| f.read_dir(d)).map_err(fat_error)?;
        Ok(entries.iter().map(|e| self.info(dir, e)).collect())
    }

    fn read(&self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let e = self.ids.lock().get(&id).cloned().ok_or(Error::NotFound)?;
        if e.is_dir {
            return Err(Error::IsDir);
        }
        self.fat
            .with(|f| f.read_file(&e, offset, buf))
            .map_err(fat_error)
    }

    fn write(&self, _: u64, _: u64, _: &[u8]) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }

    fn create(&self, _: u64, _: &str, _: bool) -> Result<NodeInfo, Error> {
        Err(Error::ReadOnly)
    }

    fn remove(&self, _: u64, _: &str) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }

    fn rename(&self, _: u64, _: &str, _: u64, _: &str) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }

    fn set_size(&self, _: u64, _: u64) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }

    fn read_only(&self) -> bool {
        true
    }

    fn space(&self) -> (u64, u64) {
        (self.fat.with(|f| f.size()), 0)
    }

    fn kind(&self) -> u32 {
        abi::volume_kind::FAT
    }
}

// ---- The initrd ----

/// The initial RAM disk as a read-only volume (`Initrd`): the ustar
/// archive the loader brought (`frame::mm::initrd`). Its files are in the
/// root folder (paths inside the archive keep their `/`, so `a/b` is
/// reached as folder `a`, file `b`); ids are 2 + the member's index.
pub struct InitrdFs {
    archive: &'static [u8],
}

const INITRD_ROOT: u64 = 1;

impl InitrdFs {
    /// The volume over `archive`.
    pub fn new(archive: &'static [u8]) -> InitrdFs {
        InitrdFs { archive }
    }

    fn members(&self) -> impl Iterator<Item = (u64, crate::initrd::Entry<'static>)> {
        crate::initrd::entries(self.archive)
            .map_while(Result::ok)
            .enumerate()
            .filter(|(_, e)| e.kind == crate::initrd::Kind::File)
            .map(|(i, e)| (i as u64 + 2, e))
    }

    fn info(id: u64, e: &crate::initrd::Entry<'_>) -> NodeInfo {
        let full = if e.prefix.is_empty() {
            String::from(e.name)
        } else {
            alloc::format!("{}/{}", e.prefix, e.name)
        };
        let name = full.rsplit('/').next().unwrap_or("").into();
        NodeInfo {
            id,
            is_dir: false,
            size: e.data.len() as u64,
            ctime: 0,
            mtime: 0,
            name,
        }
    }
}

impl FileSystem for InitrdFs {
    fn root(&self) -> u64 {
        INITRD_ROOT
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<NodeInfo, Error> {
        if dir != INITRD_ROOT {
            return Err(Error::NotDir);
        }
        self.members()
            .find(|(_, e)| e.is(name) || e.name.eq_ignore_ascii_case(name))
            .map(|(id, e)| Self::info(id, &e))
            .ok_or(Error::NotFound)
    }

    fn get(&self, id: u64) -> Result<NodeInfo, Error> {
        if id == INITRD_ROOT {
            return Ok(NodeInfo {
                id,
                is_dir: true,
                size: 0,
                ctime: 0,
                mtime: 0,
                name: String::new(),
            });
        }
        self.members()
            .find(|(i, _)| *i == id)
            .map(|(i, e)| Self::info(i, &e))
            .ok_or(Error::NotFound)
    }

    fn list(&self, dir: u64) -> Result<Vec<NodeInfo>, Error> {
        if dir != INITRD_ROOT {
            return Err(Error::NotDir);
        }
        Ok(self.members().map(|(id, e)| Self::info(id, &e)).collect())
    }

    fn read(&self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let (_, e) = self
            .members()
            .find(|(i, _)| *i == id)
            .ok_or(Error::NotFound)?;
        let data = e.data;
        if offset >= data.len() as u64 {
            return Ok(0);
        }
        let n = (data.len() - offset as usize).min(buf.len());
        buf[..n].copy_from_slice(&data[offset as usize..offset as usize + n]);
        Ok(n)
    }

    fn write(&self, _: u64, _: u64, _: &[u8]) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }

    fn create(&self, _: u64, _: &str, _: bool) -> Result<NodeInfo, Error> {
        Err(Error::ReadOnly)
    }

    fn remove(&self, _: u64, _: &str) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }

    fn rename(&self, _: u64, _: &str, _: u64, _: &str) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }

    fn set_size(&self, _: u64, _: u64) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }

    fn read_only(&self) -> bool {
        true
    }

    fn space(&self) -> (u64, u64) {
        (self.archive.len() as u64, 0)
    }

    fn kind(&self) -> u32 {
        abi::volume_kind::INITRD
    }
}
