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
    if let Err(e) = task::spawn("usb", move || drivers::usb::xhci::run(controller)) {
        frame::kprintln!("usb: cannot start the thread: {e:?}");
    }
}
