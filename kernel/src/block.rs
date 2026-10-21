// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The block devices found at boot (`docs/storage.md`): the disk drivers
//! in `drivers` are given the scheduler (so that requests sleep instead of
//! spinning), probed, and their disks kept here by name.

use crate::task;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use block::BlockDevice;
use frame::sync::SpinLock;

/// A disk the kernel can use from any thread.
pub type Disk = Arc<dyn BlockDevice + Send + Sync>;

static DISKS: SpinLock<Vec<(String, Disk)>> = SpinLock::new(Vec::new());

/// Hands the drivers the scheduler's functions, then probes the AHCI and
/// NVMe controllers. Call once at boot, from a thread, after PCI.
pub fn probe() {
    drivers::sync::set_hooks(drivers::sync::Hooks {
        current: task::current_id,
        block_until: task::block_until,
        wake: task::wake,
        now_ns: frame::time::now_ns,
    });
    let mut found = drivers::ahci::probe();
    found.extend(drivers::nvme::probe());
    let mut disks = DISKS.lock();
    for (name, disk) in found {
        disks.push((name, Arc::from(disk)));
    }
}

/// The disks, in probe order.
pub fn disks() -> Vec<(String, Disk)> {
    DISKS.lock().clone()
}

/// A disk arrived (a USB disk, P11 K41): kept, and its volumes mounted by
/// a thread of its own (mounting reads the disk, which the USB thread
/// that calls this serves).
pub fn attach(name: String, disk: Disk) {
    DISKS.lock().push((name.clone(), Arc::clone(&disk)));
    if let Err(e) = task::spawn("mount", move || {
        crate::vfs::mount::mount_removable(&name, disk)
    }) {
        frame::kprintln!("block: cannot mount the new disk: {e:?}");
    }
}

/// The disk `name` went away: its volumes are unmounted.
pub fn detach(name: &str) {
    DISKS.lock().retain(|(n, _)| n != name);
    crate::vfs::mount::unmount_removable(name);
}
