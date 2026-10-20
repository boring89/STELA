// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mounting at boot (`docs/vfs.md`): every disk is read for a GPT; the
//! EFI System Partition (by its type GUID) becomes the read-only FAT
//! volume `Boot`, and every eHFS partition (found by its superblock magic,
//! whatever its type GUID) is mounted under its volume name; the first
//! writable eHFS volume is `Data` (a volume named otherwise is renamed,
//! with a warning). A disk without a GPT that starts with an eHFS
//! superblock is mounted whole. Partitions must start on a 4 KiB
//! boundary. The initrd is mounted as the read-only `Initrd`, then the
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

/// Mounts what the disks hold and prints one line per volume.
pub fn mount_all() {
    let mut have_data = false;
    let mut summary: Vec<String> = Vec::new();
    for (disk_name, disk) in disks() {
        let parts: Vec<(Option<Guid>, u64, u64)> = match gpt::read(&*disk) {
            Ok(table) => table
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
                .collect(),
            Err(_) => vec![(None, 0, disk.block_count())],
        };
        for (kind, start, count) in parts {
            let Some(part) = Partition::new(Arc::clone(&disk), start, count, false) else {
                continue;
            };
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
    if summary.is_empty() {
        frame::kprintln!("vfs: no volumes");
    } else {
        frame::kprintln!("vfs: mounted {}", summary.join(", "));
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
        match FatFs::mount(device) {
            Ok((fs, _label)) => {
                let (total, _) = super::FileSystem::space(&fs);
                let volume = mount("Boot", Arc::new(fs));
                return Some(alloc::format!("{} (FAT, ro, {})", volume.name, mib(total)));
            }
            Err(e) => frame::kprintln!("vfs: {disk_name}: EFI system partition not mounted: {e:?}"),
        }
    }
    None
}
