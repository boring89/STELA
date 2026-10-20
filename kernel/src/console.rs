// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The console service: a kernel thread that owns a port and prints the
//! text of every `CONSOLE_WRITE` message sent to it. Every process gets a
//! `SEND` handle to it as bootstrap handle `[1]`; libstela's
//! `debug_print!` sends there.

use crate::object::port::{KERNEL_OWNER, Port};
use crate::task;
use abi::{MESSAGE_HANDLES, tag};
use alloc::sync::Arc;
use frame::sync::SpinLock;

static SERVICE: SpinLock<Option<Arc<Port>>> = SpinLock::new(None);

/// The service's port, once [`start`] ran.
pub fn service_port() -> Option<Arc<Port>> {
    SERVICE.lock().clone()
}

/// Creates the service port and starts the `console` thread. Call once at
/// boot, before the first process.
pub fn start() {
    let port = Port::new(KERNEL_OWNER);
    *SERVICE.lock() = Some(Arc::clone(&port));
    if let Err(e) = task::spawn("console", move || serve(&port)) {
        panic!("console: cannot start the service thread: {e:?}");
    }
}

fn serve(port: &Port) {
    loop {
        // Handles sent here by mistake are taken and released.
        let Ok(queued) = port.receive(None, MESSAGE_HANDLES) else {
            continue;
        };
        if queued.msg.tag != tag::CONSOLE_WRITE {
            continue;
        }
        for chunk in queued.msg.payload().utf8_chunks() {
            frame::kprint!("{}", chunk.valid());
            if !chunk.invalid().is_empty() {
                frame::kprint!("\u{FFFD}");
            }
        }
    }
}
