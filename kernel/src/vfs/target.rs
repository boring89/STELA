// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The kernel side of the VFS: the backend trait, volumes, nodes and the
//! eHFS and FAT backends.

use crate::block::Disk;
use crate::fs::cache::CachedDevice;
use crate::fs::fat::{Dir, FatEntry, FatError, FatVolume, fat_now};
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
    /// The item's 32 bytes of Finder information.
    fn get_info(&self, _id: u64) -> Result<[u8; 32], Error> {
        Err(Error::NotSupported)
    }
    /// P14 K2: `AccessDenied` when item `name` of `dir` is locked (the
    /// Finder flag `kNameLocked`, `abi::finder`): it is not deleted,
    /// renamed or moved. A missing item passes (the operation says so).
    fn check_unlocked(&self, dir: u64, name: &str) -> Result<(), Error> {
        let Ok(item) = self.lookup(dir, name) else {
            return Ok(());
        };
        match self.get_info(item.id) {
            Ok(info) if abi::finder::locked(&info) => Err(Error::AccessDenied),
            _ => Ok(()),
        }
    }
    /// Stores the item's Finder information.
    fn set_info(&self, _id: u64, _info: &[u8; 32]) -> Result<(), Error> {
        Err(Error::NotSupported)
    }
    /// The value of the item's extended attribute `name` (P11).
    fn get_attr(&self, _id: u64, _name: &str) -> Result<Vec<u8>, Error> {
        Err(Error::NotSupported)
    }
    /// Sets (`Some`) or removes (`None`) an extended attribute.
    fn set_attr(&self, _id: u64, _name: &str, _value: Option<&[u8]>) -> Result<(), Error> {
        Err(Error::NotSupported)
    }
    /// The names of the item's extended attributes.
    fn list_attrs(&self, _id: u64) -> Result<Vec<String>, Error> {
        Err(Error::NotSupported)
    }
    /// The item of folder `dir` after `cookie` (0: the first) and the
    /// cookie for the next call (never 0), or `None` past the last
    /// (`DirRead`, P11). The default counts positions in `list` (each call
    /// lists the folder again); eHFS walks its B-tree from the last name.
    fn list_after(&self, dir: u64, cookie: u64) -> Result<Option<(NodeInfo, u64)>, Error> {
        let list = self.list(dir)?;
        Ok(usize::try_from(cookie)
            .ok()
            .and_then(|i| list.into_iter().nth(i))
            .map(|info| (info, cookie + 1)))
    }
    /// The folder item `id` is in (`NotFound` for the root). Backends
    /// without parent links search from the root (P11, `FilePath`).
    fn parent(&self, id: u64) -> Result<u64, Error> {
        let root = self.root();
        if id == root {
            return Err(Error::NotFound);
        }
        let mut queue = alloc::collections::VecDeque::from([root]);
        let mut looked = 0;
        while let Some(dir) = queue.pop_front() {
            for item in self.list(dir)? {
                if item.id == id {
                    return Ok(dir);
                }
                if item.is_dir {
                    queue.push_back(item.id);
                }
            }
            looked += 1;
            if looked > 100_000 {
                break;
            }
        }
        Err(Error::NotFound)
    }
}

/// The path of item `id` inside `volume` (`/` for the root, `/a/b`
/// otherwise), from its parents.
pub fn path_of(volume: &Volume, id: u64) -> Result<String, Error> {
    let fs = &volume.fs;
    let mut names = Vec::new();
    let mut at = id;
    while at != fs.root() {
        names.push(fs.get(at)?.name);
        at = fs.parent(at)?;
        if names.len() > 256 {
            return Err(Error::IoError);
        }
    }
    let mut path = String::new();
    for name in names.iter().rev() {
        path.push('/');
        path.push_str(name);
    }
    if path.is_empty() {
        path.push('/');
    }
    Ok(path)
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
/// The next volume's index: indices are never reused, since volumes come
/// and go (P11 K41: USB disks).
static NEXT_INDEX: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Ports told about volumes coming and going (`VolumeWatch`, P11 K42).
static WATCHERS: SpinLock<Vec<Arc<crate::object::port::Port>>> = SpinLock::new(Vec::new());

/// Most ports `VolumeWatch` keeps.
pub const MAX_WATCHERS: usize = 16;

/// Adds `port` to the ports told about volumes (closed ones are dropped
/// first). `Full` past [`MAX_WATCHERS`].
pub fn watch(port: Arc<crate::object::port::Port>) -> Result<(), Error> {
    let mut w = WATCHERS.lock();
    w.retain(|p| !p.is_closed());
    if w.len() >= MAX_WATCHERS {
        return Err(Error::Full);
    }
    w.push(port);
    Ok(())
}

/// Tells the watchers that volume `name` came (`kind` 1) or went (2).
fn notify(kind: u8, name: &str) {
    let ports: Vec<_> = WATCHERS.lock().clone();
    let mut data = alloc::vec![kind];
    data.extend_from_slice(&name.as_bytes()[..name.len().min(abi::MESSAGE_DATA - 1)]);
    for p in ports {
        let msg = abi::Message::with_data(abi::tag::VOLUME_CHANGED, &data);
        let _ = p.send(crate::object::port::QueuedMessage::plain(msg));
    }
}

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
        index: NEXT_INDEX.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        name: unique,
        fs,
    });
    volumes.push(Arc::clone(&volume));
    drop(volumes);
    notify(1, &volume.name);
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

/// Takes the volume called `name` away (P11 K41: its disk was unplugged
/// or is going): new opens do not find it; handles already open keep it
/// and get `IoError` from a gone disk. Flushes it first (an unplugged disk
/// fails that, which is printed).
pub fn unmount(name: &str) -> Option<Arc<Volume>> {
    let volume = {
        let mut volumes = VOLUMES.lock();
        let at = volumes
            .iter()
            .position(|v| v.name.eq_ignore_ascii_case(name))?;
        volumes.remove(at)
    };
    if let Err(e) = volume.fs.unmount() {
        frame::kprintln!("vfs: unmounting {}: {}", volume.name, e.name());
    }
    notify(2, &volume.name);
    Some(volume)
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
    /// The names of the items `list_after` returned last (cookie = CNID),
    /// so that a listing goes on after its last item was removed.
    cursors: SpinLock<BTreeMap<u64, Vec<u8>>>,
}

/// Names `EhfsFs` remembers for listings in progress.
const CURSORS: usize = 256;

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
                cursors: SpinLock::new(BTreeMap::new()),
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

    fn parent(&self, id: u64) -> Result<u64, Error> {
        if id == self.root() {
            return Err(Error::NotFound);
        }
        self.with(|v| v.get(id)).map(|e| e.parent)
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

    fn list_after(&self, dir: u64, cookie: u64) -> Result<Option<(NodeInfo, u64)>, Error> {
        // The cookie is the CNID of the item returned last.
        let after = if cookie == 0 {
            None
        } else {
            let remembered = self.cursors.lock().get(&cookie).cloned();
            match remembered {
                Some(name) => Some(name),
                None => Some(self.with(|v| v.get(cookie))?.name),
            }
        };
        let items = self.with(|v| {
            if !v.get(dir)?.is_dir {
                return Err(ehfs::Error::NotDir);
            }
            v.list_after(dir, after.as_deref(), 1)
        })?;
        let Some(e) = items.into_iter().next() else {
            return Ok(None);
        };
        let mut cursors = self.cursors.lock();
        if cursors.len() >= CURSORS {
            cursors.pop_first();
        }
        cursors.insert(e.cnid, e.name.clone());
        Ok(Some((ehfs_info(&e), e.cnid)))
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

    fn get_info(&self, id: u64) -> Result<[u8; 32], Error> {
        self.with(|v| v.get(id)).map(|e| e.finder)
    }

    fn set_info(&self, id: u64, info: &[u8; 32]) -> Result<(), Error> {
        self.with(|v| {
            let mut e = v.get(id)?;
            e.finder = *info;
            v.set_info(&e)
        })
    }

    fn get_attr(&self, id: u64, name: &str) -> Result<Vec<u8>, Error> {
        self.with(|v| v.attr_get(id, name.as_bytes()))
    }

    fn set_attr(&self, id: u64, name: &str, value: Option<&[u8]>) -> Result<(), Error> {
        self.with(|v| match value {
            Some(value) => v.attr_set(id, name.as_bytes(), value),
            None => v.attr_remove(id, name.as_bytes()),
        })
    }

    fn list_attrs(&self, id: u64) -> Result<Vec<String>, Error> {
        self.with(|v| v.attr_names(id)).map(|names| {
            names
                .into_iter()
                .map(|n| String::from_utf8_lossy(&n).into_owned())
                .collect()
        })
    }
}

// ---- FAT ----

fn fat_error(e: FatError) -> Error {
    match e {
        FatError::Device(_) | FatError::Corrupt | FatError::NotFat | FatError::Fat12 => {
            Error::IoError
        }
        FatError::Exists => Error::Exists,
        FatError::NotFound => Error::NotFound,
        FatError::NotEmpty => Error::NotEmpty,
        FatError::NoSpace => Error::NoSpace,
        FatError::BadName => Error::InvalidArgument,
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

/// A FAT volume (P11 K42: writable on USB disks; the EFI System Partition
/// stays read only). FAT has no persistent ids: entries get ids as they
/// are seen (stable while the system runs).
pub struct FatFs {
    fat: SleepMutex<FatVolume<VolumeDevice>>,
    read_only: bool,
    /// id -> (entry, the folder it is in).
    ids: SpinLock<BTreeMap<u64, FatEntry>>,
    /// (folder id, upper-case name) -> id.
    names: SpinLock<BTreeMap<(u64, String), u64>>,
}

const FAT_ROOT: u64 = 1;

impl FatFs {
    /// Mounts the FAT volume on `device`.
    pub fn mount(device: VolumeDevice, read_only: bool) -> Result<(FatFs, String), FatError> {
        let fat = FatVolume::mount(device)?;
        let label = String::from(fat.label());
        Ok((
            FatFs {
                fat: SleepMutex::new(fat),
                read_only,
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

    fn write(&self, id: u64, offset: u64, data: &[u8]) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        let mut e = self.ids.lock().get(&id).cloned().ok_or(Error::NotFound)?;
        let now = fat_now(frame::time::wall_clock_ns());
        self.fat
            .with(|f| f.write_file(&mut e, offset, data, now))
            .map_err(fat_error)?;
        self.ids.lock().insert(id, e);
        Ok(())
    }

    fn create(&self, dir: u64, name: &str, is_dir: bool) -> Result<NodeInfo, Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        let d = self.dir_of(dir)?;
        let now = fat_now(frame::time::wall_clock_ns());
        let e = self
            .fat
            .with(|f| f.create(d, name, is_dir, now))
            .map_err(fat_error)?;
        Ok(self.info(dir, &e))
    }

    fn remove(&self, dir: u64, name: &str) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        let d = self.dir_of(dir)?;
        self.fat.with(|f| f.remove(d, name)).map_err(fat_error)?;
        if let Some(id) = self.names.lock().remove(&(dir, name.to_ascii_uppercase())) {
            self.ids.lock().remove(&id);
        }
        Ok(())
    }

    fn rename(&self, dir: u64, name: &str, new_dir: u64, new_name: &str) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        let (d, nd) = (self.dir_of(dir)?, self.dir_of(new_dir)?);
        let now = fat_now(frame::time::wall_clock_ns());
        self.fat
            .with(|f| f.rename(d, name, nd, new_name, now))
            .map_err(fat_error)?;
        if let Some(id) = self.names.lock().remove(&(dir, name.to_ascii_uppercase())) {
            self.ids.lock().remove(&id);
        }
        Ok(())
    }

    fn set_size(&self, id: u64, size: u64) -> Result<(), Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        let mut e = self.ids.lock().get(&id).cloned().ok_or(Error::NotFound)?;
        let now = fat_now(frame::time::wall_clock_ns());
        self.fat
            .with(|f| f.set_size(&mut e, size, now))
            .map_err(fat_error)?;
        self.ids.lock().insert(id, e);
        Ok(())
    }

    fn read_only(&self) -> bool {
        self.read_only
    }

    fn space(&self) -> (u64, u64) {
        self.fat.with(|f| (f.size(), f.free_bytes().unwrap_or(0)))
    }

    fn kind(&self) -> u32 {
        abi::volume_kind::FAT
    }
}

// ---- The initrd ----

/// The initial RAM disk as a read-only volume (`Initrd`): the ustar
/// archive the loader brought (`frame::mm::initrd`), as a folder tree
/// (`crate::initrd::Tree`: `linux/hello` is file `hello` in folder
/// `linux`).
pub struct InitrdFs {
    archive: &'static [u8],
    tree: crate::initrd::Tree<'static>,
}

impl InitrdFs {
    /// The volume over `archive`.
    pub fn new(archive: &'static [u8]) -> InitrdFs {
        InitrdFs {
            archive,
            tree: crate::initrd::Tree::new(archive),
        }
    }

    fn info(n: &crate::initrd::TreeNode<'_>) -> NodeInfo {
        NodeInfo {
            id: n.id,
            is_dir: n.is_dir,
            size: n.data.len() as u64,
            ctime: 0,
            mtime: 0,
            name: n.name.clone(),
        }
    }

    fn folder(&self, dir: u64) -> Result<(), Error> {
        match self.tree.get(dir) {
            Some(n) if n.is_dir => Ok(()),
            Some(_) => Err(Error::NotDir),
            None => Err(Error::NotFound),
        }
    }
}

impl FileSystem for InitrdFs {
    fn root(&self) -> u64 {
        crate::initrd::TREE_ROOT
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<NodeInfo, Error> {
        self.folder(dir)?;
        self.tree
            .lookup(dir, name)
            .map(Self::info)
            .ok_or(Error::NotFound)
    }

    fn get(&self, id: u64) -> Result<NodeInfo, Error> {
        self.tree.get(id).map(Self::info).ok_or(Error::NotFound)
    }

    fn list(&self, dir: u64) -> Result<Vec<NodeInfo>, Error> {
        self.folder(dir)?;
        Ok(self.tree.children(dir).map(Self::info).collect())
    }

    fn read(&self, id: u64, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let n = self.tree.get(id).ok_or(Error::NotFound)?;
        if n.is_dir {
            return Err(Error::IsDir);
        }
        let data = n.data;
        if offset >= data.len() as u64 {
            return Ok(0);
        }
        let len = (data.len() - offset as usize).min(buf.len());
        buf[..len].copy_from_slice(&data[offset as usize..offset as usize + len]);
        Ok(len)
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
