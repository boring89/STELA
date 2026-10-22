// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The System volume's slots (P14 K5, `docs/system.md` §4–§5): which of
//! the two STELA System partitions to mount, the check of its image (the
//! trailer's signature, and the whole image's SHA-256 after an update or
//! a start that did not reach the desktop), the slot file on the ESP
//! (`STELA/slot`) and `BootOk`. The logic is `sysimage`'s (host tests);
//! this module reads the disks and the ESP.

use crate::block::Disk;
use crate::vfs::FatFs;
use abi::Error;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use block::{BLOCK_SIZE, BlockDevice, Partition};
use frame::sync::SpinLock;
use sysimage::{Slot, SlotFile};

/// The key System images are signed with: `tools/system.pub`, the TEST
/// System key (not the package key; `docs/system.md` §3; replace before
/// releasing).
const KEY_HEX: &str = include_str!("../../tools/system.pub");

/// Where the slot file is on the ESP.
const SLOT_FOLDER: &str = "STELA";
const SLOT_NAME: &str = "slot";

/// A place that may hold a System image.
#[derive(Clone)]
pub struct Place {
    /// The disk's name (`ahci0`).
    pub disk_name: String,
    /// The disk.
    pub disk: Disk,
    /// First block and blocks (4 KiB).
    pub start: u64,
    /// Blocks.
    pub count: u64,
}

impl Place {
    fn partition(&self) -> Option<Partition<Disk>> {
        Partition::new(Arc::clone(&self.disk), self.start, self.count, false)
    }

    /// The place, read only (an installation copies it).
    pub fn partition_for_copy(&self) -> Option<Partition<Disk>> {
        Partition::new(Arc::clone(&self.disk), self.start, self.count, true)
    }
}

/// What the kernel knows about the System slots.
struct State {
    /// The ESP's FAT (for the slot file), when there is one.
    boot: Option<Arc<FatFs>>,
    /// Where that ESP is (P14 K7: Setup copies it).
    boot_place: Option<Place>,
    /// Slot A and B (`None`: no such partition).
    slots: [Option<Place>; 2],
    /// Their images' versions, when their trailers verified.
    versions: [Option<String>; 2],
    /// The slot mounted (`None`: no System, or one without slots).
    mounted: Option<Slot>,
    /// The slot file as it is now.
    file: Option<SlotFile>,
}

static STATE: SpinLock<State> = SpinLock::new(State {
    boot: None,
    boot_place: None,
    slots: [None, None],
    versions: [None, None],
    mounted: None,
    file: None,
});

/// Records the ESP's FAT volume and where it is (`vfs::mount`).
pub fn set_boot(fat: Arc<FatFs>, place: Place) {
    let mut s = STATE.lock();
    if s.boot.is_none() {
        s.boot = Some(fat);
        s.boot_place = Some(place);
    }
}

/// What an installation copies (P14 K7, `vfs::mount::install`): the ESP
/// this start came from, the mounted System slot and how many of its
/// blocks hold the image (the volume and the trailer).
pub fn install_sources() -> Result<(Place, Place, u64), Error> {
    let (esp, system) = {
        let s = STATE.lock();
        let mounted = s.mounted.ok_or(Error::NotSupported)?;
        (
            s.boot_place.clone().ok_or(Error::NotFound)?,
            s.slots[mounted.index()].clone().ok_or(Error::NotFound)?,
        )
    };
    let part = system.partition().ok_or(Error::IoError)?;
    let mut block = vec![0u8; BLOCK_SIZE];
    part.read(0, &mut block).map_err(|_| Error::IoError)?;
    let blocks = ehfs::block_count(&block).ok_or(Error::IoError)?;
    Ok((esp, system, blocks + 1))
}

fn key() -> [u8; 32] {
    // A broken key file is a build error caught by the host tests.
    sysimage::key_from_hex(KEY_HEX).unwrap_or([0; 32])
}

/// Why an image was not used.
#[derive(Debug)]
pub enum Bad {
    /// Block 0 is not an eHFS volume called System.
    NotSystem,
    /// No trailer after the volume.
    NoTrailer,
    /// The trailer's signature is not the key's.
    Signature,
    /// The image's bytes do not hash to the trailer's SHA-256.
    Hash,
    /// The disk could not be read.
    Io,
}

/// Checks the image at `part`: its trailer (right after the eHFS volume)
/// must be signed; with `full`, every byte before it must hash to the
/// trailer's SHA-256. Returns the version.
pub fn check(part: &Partition<Disk>, full: bool) -> Result<String, Bad> {
    let mut block = vec![0u8; BLOCK_SIZE];
    part.read(0, &mut block).map_err(|_| Bad::Io)?;
    if ehfs::label(&block).as_deref() != Some(crate::vfs::mount::SYSTEM) {
        return Err(Bad::NotSystem);
    }
    let blocks = ehfs::block_count(&block).ok_or(Bad::NotSystem)?;
    if blocks >= part.block_count() {
        return Err(Bad::NoTrailer);
    }
    let mut trailer = vec![0u8; BLOCK_SIZE];
    part.read(blocks, &mut trailer).map_err(|_| Bad::Io)?;
    let t = sysimage::parse(&trailer).ok_or(Bad::NoTrailer)?;
    if !sysimage::verify(&trailer, &key()) || t.data_len != blocks * BLOCK_SIZE as u64 {
        return Err(Bad::Signature);
    }
    if full {
        let mut h = sysimage::Hasher::new();
        // 64 KiB at a time.
        let mut buf = vec![0u8; 16 * BLOCK_SIZE];
        let mut at = 0;
        while at < blocks {
            let n = (blocks - at).min(16);
            let piece = &mut buf[..n as usize * BLOCK_SIZE];
            part.read(at, piece).map_err(|_| Bad::Io)?;
            h.update(piece);
            at += n;
        }
        if h.finish() != t.sha256 {
            return Err(Bad::Hash);
        }
    }
    Ok(t.version)
}

/// The slot file, read from the ESP.
fn read_file(boot: &Option<Arc<FatFs>>) -> Option<SlotFile> {
    let data = boot.as_ref()?.kernel_read(SLOT_FOLDER, SLOT_NAME, 64)?;
    SlotFile::parse(core::str::from_utf8(&data).ok()?)
}

/// Writes the slot file and keeps it (the disk is written without the
/// lock held).
fn write_file(file: SlotFile) -> Result<(), Error> {
    let boot = STATE.lock().boot.clone().ok_or(Error::NotFound)?;
    boot.kernel_write(SLOT_FOLDER, SLOT_NAME, file.text().as_bytes())?;
    STATE.lock().file = Some(file);
    Ok(())
}

/// The slot `forced` by `boot.cfg`'s `system=a|b` (`auto` or none: none).
fn forced_slot() -> Option<Slot> {
    let value = abi::sysinfo::cmdline_option(frame::cmdline(), "system")?;
    Slot::from_letter(core::str::from_utf8(value).ok()?)
}

/// Picks the System volume at boot (`vfs::mount::mount_all`): `typed`
/// are the STELA System partitions (slot A, slot B, in table order),
/// `named` the volumes called System found otherwise (a whole disk, as
/// QEMU's tests attach it). Returns the partition to mount and its
/// version. The disks are read without the state's lock held.
pub fn choose(typed: Vec<Place>, named: Vec<Place>) -> Option<(Partition<Disk>, String)> {
    let verify = frame::boot_flag("verify");
    if typed.is_empty() {
        // No slots: the first volume called System, checked the same way.
        for place in named {
            let part = place.partition()?;
            match check(&part, verify) {
                Ok(version) => {
                    frame::kprintln!("system: {} (version {version})", place.disk_name);
                    return Some((part, version));
                }
                Err(why) => frame::kprintln!("system: {}: not used ({why:?})", place.disk_name),
            }
        }
        return None;
    }
    let slots: Vec<Place> = typed.into_iter().take(2).collect();
    let boot = STATE.lock().boot.clone();
    let file = read_file(&boot);
    let forced = forced_slot();
    let plan = sysimage::plan(file, forced, verify);
    if plan.gave_up {
        frame::kprintln!(
            "system: slot {} did not reach the desktop {} times; starting slot {}",
            plan.first.other().letter(),
            sysimage::MAX_TRIES,
            plan.first.letter()
        );
    }
    let mut versions: [Option<String>; 2] = [None, None];
    let mut chosen = None;
    for slot in [plan.first, plan.first.other()] {
        let Some(part) = slots.get(slot.index()).and_then(Place::partition) else {
            continue;
        };
        // The fallback is checked in full too: it is used because
        // something went wrong.
        let full = plan.full_check || slot != plan.first;
        match check(&part, full) {
            Ok(version) => {
                versions[slot.index()] = Some(version.clone());
                chosen = Some((slot, part, version, full));
                break;
            }
            Err(why) => frame::kprintln!(
                "system: slot {}: not used ({why:?}){}",
                slot.letter(),
                if slot == plan.first {
                    "; trying the other slot"
                } else {
                    ""
                }
            ),
        }
    }
    {
        let mut s = STATE.lock();
        s.file = file;
        for (i, place) in slots.iter().enumerate() {
            s.slots[i] = Some(place.clone());
        }
    }
    let (slot, part, version, full) = chosen?;
    // The other slot's version, for `stela-update status` (signature only).
    let other = slot.other();
    if let Some(p) = slots.get(other.index()).and_then(Place::partition) {
        versions[other.index()] = check(&p, false).ok();
    }
    {
        let mut s = STATE.lock();
        s.mounted = Some(slot);
        s.versions = versions;
    }
    if let Some(new) = sysimage::after_start(file, slot, forced.is_some())
        && let Err(e) = write_file(new)
    {
        frame::kprintln!("system: cannot write the slot file: {}", e.name());
    }
    frame::kprintln!(
        "system: slot {} (version {version}{})",
        slot.letter(),
        if full { ", checked in full" } else { "" }
    );
    Some((part, version))
}

/// `BootOk` (the desktop is up): the start is no longer a try.
pub fn boot_ok() -> Result<(), Error> {
    let Some(file) = STATE.lock().file else {
        return Ok(());
    };
    if file.tries == 0 && !file.fresh {
        return Ok(());
    }
    write_file(sysimage::confirmed(file))?;
    frame::kprintln!("system: slot {} confirmed", file.slot.letter());
    Ok(())
}

/// `PartitionWrite` (P14 K6): `data` at byte `offset` of the slot that is
/// **not** mounted (whole 4 KiB blocks). A write at offset 0 first points
/// the slot file back at the mounted slot when it pointed at the one
/// being written, so that a write cut short is never started.
pub fn write_other(offset: u64, data: &[u8]) -> Result<(), Error> {
    let block = BLOCK_SIZE as u64;
    if !offset.is_multiple_of(block) || !data.len().is_multiple_of(BLOCK_SIZE) || data.is_empty() {
        return Err(Error::InvalidArgument);
    }
    let (place, mounted, file) = {
        let s = STATE.lock();
        let mounted = s.mounted.ok_or(Error::NotSupported)?;
        let place = s.slots[mounted.other().index()]
            .clone()
            .ok_or(Error::NotFound)?;
        (place, mounted, s.file)
    };
    if offset == 0
        && let Some(f) = file
        && f.slot != mounted
    {
        write_file(SlotFile {
            slot: mounted,
            tries: 0,
            fresh: false,
        })?;
    }
    let part = place.partition().ok_or(Error::IoError)?;
    let first = offset / block;
    if first + (data.len() / BLOCK_SIZE) as u64 > part.block_count() {
        return Err(Error::InvalidArgument);
    }
    STATE.lock().versions[mounted.other().index()] = None;
    part.write(first, data).map_err(|_| Error::IoError)
}

/// `SystemUse(which)` (P14 K6): start from the slot that is not mounted
/// (`which` 0; its image is checked in full first: `InvalidArgument` when
/// it is not a good System image) or from the mounted one (1: undoes a
/// pending switch). Writes the slot file; returns the slot's version.
pub fn use_slot(which: u64) -> Result<String, Error> {
    let (mounted, place) = {
        let s = STATE.lock();
        let mounted = s.mounted.ok_or(Error::NotSupported)?;
        let target = if which == 0 { mounted.other() } else { mounted };
        (
            mounted,
            s.slots[target.index()].clone().ok_or(Error::NotFound)?,
        )
    };
    let target = if which == 0 { mounted.other() } else { mounted };
    let part = place.partition().ok_or(Error::IoError)?;
    let version = check(&part, which == 0).map_err(|why| {
        frame::kprintln!("system: slot {}: not used ({why:?})", target.letter());
        Error::InvalidArgument
    })?;
    write_file(SlotFile {
        slot: target,
        tries: 0,
        fresh: target != mounted,
    })?;
    STATE.lock().versions[target.index()] = Some(version.clone());
    frame::kprintln!(
        "system: the next start uses slot {} (version {version})",
        target.letter()
    );
    Ok(version)
}

/// `SystemStatus` text: `mounted=a slot=a 0 a=0.1.0 b=0.1.1` (`-` for
/// none).
pub fn status() -> String {
    let s = STATE.lock();
    let word = |v: &Option<String>| v.clone().unwrap_or_else(|| String::from("-"));
    alloc::format!(
        "mounted={} slot={} a={} b={}\n",
        s.mounted.map_or('-', Slot::letter),
        s.file
            .map_or_else(|| String::from("-"), |f| String::from(f.text().trim_end())),
        word(&s.versions[0]),
        word(&s.versions[1]),
    )
}
