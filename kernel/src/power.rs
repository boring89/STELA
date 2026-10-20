// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shutting down and restarting (`docs/acpi.md`): the volumes are flushed
//! and marked clean first, then `frame::power` does the rest.

use frame::power::{self, PowerError};

/// Unmounts every volume and turns the machine off. Returns only if that
/// is not possible (`NotSupported`: nothing was unmounted) or did not work
/// (`Failed`: the volumes are already marked clean; their journal still
/// protects them if writing goes on).
pub fn shutdown() -> PowerError {
    if !power::can_shutdown() {
        return PowerError::NotSupported;
    }
    crate::vfs::unmount_all();
    frame::kprintln!("kernel: volumes unmounted");
    power::shutdown()
}

/// Unmounts every volume and restarts the machine.
pub fn reboot() -> ! {
    crate::vfs::unmount_all();
    frame::kprintln!("kernel: volumes unmounted");
    power::reboot()
}
