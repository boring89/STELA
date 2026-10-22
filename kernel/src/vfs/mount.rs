// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mounting at boot (`docs/vfs.md`): every disk is read for a GPT; the
//! EFI System Partition (by its type GUID) becomes the read-only FAT
//! volume `Boot`, and every eHFS partition (found by its superblock magic,
//! whatever its type GUID) is mounted under its volume name; the first
//! writable eHFS volume is `Data` (a volume named otherwise is renamed,
//! with a warning). A disk without a GPT that starts with an eHFS
//! superblock is mounted whole. Partitions must start on a 4 KiB
//! boundary. P11 K41: a disk without a GPT may have an MBR; USB disks are
//! mounted when they arrive (eHFS and FAT under their labels) and
//! unmounted when they go. The initrd is mounted as the read-only `Initrd`, then the
//! memory file system as `Tmp` (`tmpfs.rs`).

use super::{EhfsFs, FatFs, VolumeDevice, mount};
use crate::block::{Disk, disks};
use crate::fs::cache::{CachedDevice, DEFAULT_BLOCKS};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use block::gpt::{self, Guid};
use block::{BLOCK_SIZE, BlockDevice, Partition};

/// Blocks the cache of a FAT volume keeps.
const FAT_CACHE: usize = 64;

fn mib(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        alloc::format!("{} GiB", bytes >> 30)
    } else {
        alloc::format!("{} MiB", bytes >> 20)
    }
}

/// The partitions of `disk`: (GPT type, first block, blocks) from its
/// GPT, else (P11 K41) its MBR, else the whole disk.
fn partitions(disk_name: &str, disk: &Disk) -> Vec<(Option<Guid>, u64, u64)> {
    if let Ok(table) = gpt::read(&**disk) {
        return table
            .partitions
            .iter()
            .filter_map(|p| match table.blocks_of(p) {
                Some((start, count)) => Some((Some(p.kind), start, count)),
                None => {
                    frame::kprintln!(
                        "vfs: {disk_name} partition {}: not 4 KiB aligned; skipped",
                        p.index + 1
                    );
                    None
                }
            })
            .collect();
    }
    let mut first = vec![0u8; BLOCK_SIZE];
    if disk.read(0, &mut first).is_ok()
        && let Some(table) = block::mbr::read(&first[..512])
    {
        return table
            .iter()
            .filter_map(|p| match p.blocks() {
                Some(b) => Some((None, b.0, b.1)),
                None => {
                    frame::kprintln!(
                        "vfs: {disk_name} MBR partition {}: not 4 KiB aligned; skipped",
                        p.index + 1
                    );
                    None
                }
            })
            .collect();
    }
    vec![(None, 0, disk.block_count())]
}

/// Which volumes each removable disk has (P11 K41).
static REMOVABLE: frame::sync::SpinLock<Vec<(String, Vec<String>)>> =
    frame::sync::SpinLock::new(Vec::new());

/// Mounts the volumes of a disk that just arrived (a USB disk): eHFS and
/// FAT, under their labels. Never `Data`. Prints what it mounted.
pub fn mount_removable(disk_name: &str, disk: Disk) {
    let mut names = Vec::new();
    for (kind, start, count) in partitions(disk_name, &disk) {
        let Some(part) = Partition::new(Arc::clone(&disk), start, count, false) else {
            continue;
        };
        let mut first = vec![0u8; BLOCK_SIZE];
        if part.read(0, &mut first).is_err() {
            continue;
        }
        let mounted = if ehfs::is_ehfs(&first) {
            let device: VolumeDevice = CachedDevice::new(part, DEFAULT_BLOCKS);
            EhfsFs::mount(device, false).ok().map(|(fs, label)| {
                let what = alloc::format!("eHFS, {}", mib(super::FileSystem::space(&fs).0));
                (mount(&label, Arc::new(fs)), what)
            })
        } else {
            let device: VolumeDevice = CachedDevice::new(part, FAT_CACHE);
            FatFs::mount(device, false).ok().map(|(fs, label)| {
                let label = if label.trim().is_empty() || kind == Some(Guid::ESP) {
                    String::from(if kind == Some(Guid::ESP) {
                        "EFI"
                    } else {
                        "USB"
                    })
                } else {
                    String::from(label.trim())
                };
                let what = alloc::format!("FAT, rw, {}", mib(super::FileSystem::space(&fs).0));
                (mount(&label, Arc::new(fs)), what)
            })
        };
        match mounted {
            Some((volume, what)) => {
                frame::kprintln!("vfs: mounted {} ({what}) from {disk_name}", volume.name);
                names.push(volume.name.clone());
            }
            None => frame::kprintln!("vfs: {disk_name}: a partition without eHFS or FAT; skipped"),
        }
    }
    REMOVABLE.lock().push((String::from(disk_name), names));
}

/// Unmounts the volumes of a disk that went away (P11 K41).
pub fn unmount_removable(disk_name: &str) {
    let names = {
        let mut r = REMOVABLE.lock();
        match r.iter().position(|(d, _)| d == disk_name) {
            Some(at) => r.remove(at).1,
            None => Vec::new(),
        }
    };
    for name in names {
        if super::unmount(&name).is_some() {
            frame::kprintln!("vfs: unmounted {name} ({disk_name} went away)");
        }
    }
}

/// The name of the read-only system volume (P14 K4, `docs/system.md`).
pub const SYSTEM: &str = "System";

/// Mounts what the disks hold and prints one line per volume.
pub fn mount_all() {
    let mut have_data = false;
    let mut summary: Vec<String> = Vec::new();
    // P14 K4: System volumes wait until Initrd and Tmp are mounted, so
    // that the volume list keeps its order (Data, …, Initrd, Tmp,
    // System); one is mounted. K5: STELA System partitions are the slots
    // (crate::system chooses), whatever they hold.
    let mut typed: Vec<crate::system::Place> = Vec::new();
    let mut named: Vec<crate::system::Place> = Vec::new();
    for (disk_name, disk) in disks() {
        for (kind, start, count) in partitions(&disk_name, &disk) {
            let place = crate::system::Place {
                disk_name: disk_name.clone(),
                disk: Arc::clone(&disk),
                start,
                count,
            };
            if kind == Some(Guid::STELA_SYSTEM) {
                typed.push(place);
                continue;
            }
            let Some(part) = Partition::new(Arc::clone(&disk), start, count, false) else {
                continue;
            };
            let mut first = vec![0u8; BLOCK_SIZE];
            if part.read(0, &mut first).is_ok() && ehfs::label(&first).as_deref() == Some(SYSTEM) {
                named.push(place);
                continue;
            }
            if let Some(line) = mount_partition(&disk_name, &disk, part, kind, &mut have_data) {
                summary.push(line);
            }
        }
    }
    let archive = frame::mm::initrd();
    if !archive.is_empty() {
        let volume = mount("Initrd", Arc::new(super::InitrdFs::new(archive)));
        summary.push(alloc::format!(
            "{} (initrd, ro, {} KiB)",
            volume.name,
            archive.len() / 1024
        ));
    }
    let tmp = mount("Tmp", Arc::new(super::tmpfs::TmpFs::new()));
    summary.push(alloc::format!(
        "{} (memory, rw, {} MiB)",
        tmp.name,
        super::tmpfs::CAPACITY >> 20
    ));
    if let Some(line) = mount_system(typed, named) {
        summary.push(line);
    }
    if summary.is_empty() {
        frame::kprintln!("vfs: no volumes");
    } else {
        frame::kprintln!("vfs: mounted {}", summary.join(", "));
    }
}

/// Mounts the System volume read only as `System`: the slot
/// `crate::system::choose` picks (P14 K5), or the first volume called
/// System when there are no slots (P14 K4).
fn mount_system(
    typed: Vec<crate::system::Place>,
    named: Vec<crate::system::Place>,
) -> Option<String> {
    let (part, version) = crate::system::choose(typed, named)?;
    let device: VolumeDevice = CachedDevice::new(part, DEFAULT_BLOCKS);
    match EhfsFs::mount(device, true) {
        Ok((fs, _)) => {
            let (total, _) = super::FileSystem::space(&fs);
            let volume = mount(SYSTEM, Arc::new(fs));
            Some(alloc::format!(
                "{} (eHFS, ro, {}, {version})",
                volume.name,
                mib(total)
            ))
        }
        Err(e) => {
            frame::kprintln!("vfs: System volume not mounted: {e:?}");
            None
        }
    }
}

fn mount_partition(
    disk_name: &str,
    _disk: &Disk,
    part: Partition<Disk>,
    kind: Option<Guid>,
    have_data: &mut bool,
) -> Option<String> {
    let mut first = vec![0u8; BLOCK_SIZE];
    part.read(0, &mut first).ok()?;
    if ehfs::is_ehfs(&first) {
        let device: VolumeDevice = CachedDevice::new(part, DEFAULT_BLOCKS);
        let (fs, label) = match EhfsFs::mount(device, false) {
            Ok(r) => r,
            Err(e) => {
                frame::kprintln!("vfs: {disk_name}: eHFS volume not mounted: {e:?}");
                return None;
            }
        };
        let read_only = super::FileSystem::read_only(&fs);
        let name = if !*have_data && !read_only {
            *have_data = true;
            if label != "Data" {
                frame::kprintln!("vfs: {disk_name}: eHFS volume \"{label}\" is used as Data");
            }
            String::from("Data")
        } else {
            label
        };
        let (total, _) = super::FileSystem::space(&fs);
        let volume = mount(&name, Arc::new(fs));
        return Some(alloc::format!(
            "{} (eHFS, {}, {})",
            volume.name,
            if read_only { "ro" } else { "rw" },
            mib(total)
        ));
    }
    if kind == Some(Guid::ESP) {
        let device: VolumeDevice = CachedDevice::new(part, FAT_CACHE);
        match FatFs::mount(device, true) {
            Ok((fs, _label)) => {
                let (total, _) = super::FileSystem::space(&fs);
                // P14 K5: the kernel itself writes the slot file there.
                let fs = Arc::new(fs);
                crate::system::set_boot(Arc::clone(&fs));
                let volume = mount("Boot", fs);
                return Some(alloc::format!("{} (FAT, ro, {})", volume.name, mib(total)));
            }
            Err(e) => frame::kprintln!("vfs: {disk_name}: EFI system partition not mounted: {e:?}"),
        }
    }
    None
}

/// Whether a disk is removable (a USB disk): never offered for Data.
fn removable(disk_name: &str) -> bool {
    REMOVABLE.lock().iter().any(|(d, _)| d == disk_name) || disk_name.starts_with("usb")
}

/// The places that can become the Data volume (P12, Setup.app,
/// `docs/setup.md` §4): partitions (named `disk.N`, N from 1 in table
/// order) or whole disks without a table (named `disk`) whose first 4 KiB
/// block is all zeros (never formatted), on disks that are not removable;
/// with their size in bytes. Nothing while a Data volume is mounted.
pub fn format_candidates() -> Vec<(String, u64)> {
    if super::find("Data").is_some() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (disk_name, disk) in disks() {
        if removable(&disk_name) {
            continue;
        }
        let parts = partitions(&disk_name, &disk);
        let whole = parts.len() == 1 && parts[0].1 == 0 && parts[0].0.is_none();
        for (i, (kind, start, count)) in parts.into_iter().enumerate() {
            // P14 K5: nor the System slots (an empty slot B is all zeros).
            if kind == Some(Guid::ESP) || kind == Some(Guid::STELA_SYSTEM) {
                continue;
            }
            let Some(part) = Partition::new(Arc::clone(&disk), start, count, false) else {
                continue;
            };
            let mut first = vec![0u8; BLOCK_SIZE];
            if part.read(0, &mut first).is_err() || first.iter().any(|&b| b != 0) {
                continue;
            }
            let name = if whole {
                disk_name.clone()
            } else {
                alloc::format!("{disk_name}.{}", i + 1)
            };
            out.push((name, count * BLOCK_SIZE as u64));
        }
    }
    out
}

/// Formats the place `name` (one of [`format_candidates`]) as an eHFS
/// volume called Data and mounts it.
pub fn format_data(name: &str) -> Result<(), abi::Error> {
    if super::find("Data").is_some() {
        return Err(abi::Error::Exists);
    }
    for (disk_name, disk) in disks() {
        if removable(&disk_name) {
            continue;
        }
        let parts = partitions(&disk_name, &disk);
        let whole = parts.len() == 1 && parts[0].1 == 0 && parts[0].0.is_none();
        for (i, (kind, start, count)) in parts.into_iter().enumerate() {
            let this = if whole {
                disk_name.clone()
            } else {
                alloc::format!("{disk_name}.{}", i + 1)
            };
            if this != name || kind == Some(Guid::ESP) {
                continue;
            }
            let part = Partition::new(Arc::clone(&disk), start, count, false)
                .ok_or(abi::Error::InvalidArgument)?;
            let mut first = vec![0u8; BLOCK_SIZE];
            part.read(0, &mut first).map_err(|_| abi::Error::IoError)?;
            if first.iter().any(|&b| b != 0) {
                // Not empty (any more): never erased here.
                return Err(abi::Error::Busy);
            }
            let mut uuid = [0u8; 16];
            frame::random::fill(&mut uuid);
            let options = ehfs::FormatOptions {
                name: "Data",
                uuid,
                now: frame::time::wall_clock_ns(),
                journal_blocks: 0,
            };
            ehfs::format(part, &options).map_err(|e| {
                frame::kprintln!("vfs: formatting {name} failed: {e:?}");
                abi::Error::IoError
            })?;
            frame::kprintln!("vfs: {name} formatted as Data");
            let part = Partition::new(Arc::clone(&disk), start, count, false)
                .ok_or(abi::Error::InvalidArgument)?;
            let mut have_data = false;
            return match mount_partition(&disk_name, &disk, part, kind, &mut have_data) {
                Some(line) => {
                    frame::kprintln!("vfs: mounted {line}");
                    Ok(())
                }
                None => Err(abi::Error::IoError),
            };
        }
    }
    Err(abi::Error::NotFound)
}
