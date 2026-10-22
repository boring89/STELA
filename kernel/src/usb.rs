// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! USB (`docs/usb.md`): starts the xHCI controller, if there is one, and
//! gives it a kernel thread of its own (`usb`), which enumerates devices
//! and feeds HID keyboard and mouse reports into the input queue.

use crate::task;

/// Probes the first xHCI controller and starts its thread. Call once at
/// boot, after `block::probe` (which hands the drivers the scheduler).
/// The boot option `nousb` leaves the controller alone.
pub fn start() {
    if frame::boot_flag("nousb") {
        frame::kprintln!("usb: off (boot option nousb)");
        return;
    }
    let Some(controller) = drivers::usb::xhci::probe() else {
        return;
    };
    // USB disks (P11 K41): mounted when they come, unmounted when they go.
    drivers::usb::xhci::set_storage_hooks(drivers::usb::xhci::StorageHooks {
        attach: |name, disk| crate::block::attach(name, disk),
        detach: |name| {
            let name = alloc::string::String::from(name);
            // Not on the USB thread: unmounting may flush the disk.
            if let Err(e) = task::spawn("unmount", move || crate::block::detach(&name)) {
                frame::kprintln!("usb: cannot unmount: {e:?}");
            }
        },
    });
    // USB network adapters (P13 K10): the network thread takes them.
    drivers::usb::xhci::set_net_hooks(drivers::usb::xhci::NetHooks {
        attach: crate::net::usb_attached,
        detach: crate::net::usb_detached,
    });
    if let Err(e) = task::spawn("usb", move || drivers::usb::xhci::run(controller)) {
        frame::kprintln!("usb: cannot start the thread: {e:?}");
    }
}
