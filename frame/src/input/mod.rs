// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Input events: one queue for every input device.
//!
//! ```text
//!  IRQ 1 ──► i8042 byte ──► Decoder ──► KeyEvent ──┐
//!  (later: USB HID reports, P8) ───────────────────┤ push
//!                                                  ▼
//!                        EventQueue (256, drops the oldest when full)
//!                                                  │ pop
//!                                                  ▼
//!                          the kernel's input router (woken by the
//!                          notify hook after every push)
//! ```
//!
//! The interrupt handler pushes and the router pops; both go through a
//! [`SpinLock`], which disables interrupts while held, so the two never wait
//! for each other on one processor.

pub mod keyboard;
pub mod queue;

pub use abi::key::{KeyCode, KeyEvent, Modifiers, key_to_char};

use crate::sync::{Once, SpinLock};
use queue::EventQueue;

/// Something the user did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// A key was pressed or released.
    Key(KeyEvent),
}

/// Capacity of the event queue.
pub const QUEUE_SIZE: usize = 256;

static QUEUE: SpinLock<EventQueue<InputEvent, QUEUE_SIZE>> = SpinLock::new(EventQueue::new());

static NOTIFY: Once<fn()> = Once::new();

/// Registers a function called after every [`push`] (from the interrupt
/// handler, so it must be short: the kernel wakes a thread). Only the
/// first registration counts.
pub fn set_notify(hook: fn()) {
    let _ = NOTIFY.set(hook);
}

/// Adds an event (from a device driver or interrupt handler).
pub fn push(event: InputEvent) {
    QUEUE.lock().push(event);
    if let Some(hook) = NOTIFY.get() {
        hook();
    }
}

/// Takes the oldest event, if any.
pub fn pop() -> Option<InputEvent> {
    QUEUE.lock().pop()
}

/// Events lost because the queue was full.
pub fn dropped() -> u64 {
    QUEUE.lock().dropped()
}

#[cfg(target_os = "none")]
pub use target::{init, keyboard_present};

#[cfg(target_os = "none")]
mod target {
    use super::keyboard::Decoder;
    use super::{InputEvent, push};
    use crate::arch::x86_64::i8042;
    use crate::irq;
    use crate::sync::SpinLock;
    use crate::trap::TrapFrame;
    use core::sync::atomic::{AtomicBool, Ordering};

    static DECODER: SpinLock<Decoder> = SpinLock::new(Decoder::new());
    static PRESENT: AtomicBool = AtomicBool::new(false);

    /// The legacy IRQ of the PS/2 keyboard.
    const KEYBOARD_IRQ: u8 = 1;

    fn keyboard_interrupt(_: &TrapFrame) {
        while let Some(byte) = i8042::read_keyboard_byte() {
            let event = DECODER.lock().feed(byte);
            if let Some(event) = event {
                push(InputEvent::Key(event));
            }
        }
    }

    /// Sets up the PS/2 keyboard and routes IRQ 1 to it. A machine without
    /// an i8042 (many new PCs) just has no keyboard for now.
    pub fn init() {
        if let Err(why) = i8042::init() {
            crate::kprintln!("i8042: no PS/2 keyboard ({why})");
            return;
        }
        let vector = irq::isa_vector(KEYBOARD_IRQ);
        if let Err(e) = irq::register(vector, keyboard_interrupt) {
            panic!("i8042: cannot register vector {vector}: {e:?}");
        }
        if let Err(e) = irq::enable_isa(KEYBOARD_IRQ) {
            crate::kprintln!("i8042: cannot route IRQ {KEYBOARD_IRQ}: {e:?}");
            irq::unregister(vector);
            return;
        }
        // IRQ 1 is edge triggered: a byte that arrived before the route
        // was set up raised its edge into a masked input, and while it sits
        // in the output buffer the controller raises no new edge. Drain it.
        let mut stale = 0;
        while i8042::read_keyboard_byte().is_some() {
            stale += 1;
        }
        if stale > 0 {
            crate::kprintln!("i8042: discarded {stale} stale byte(s)");
        }
        PRESENT.store(true, Ordering::Relaxed);
        crate::kprintln!("i8042: PS/2 keyboard on IRQ {KEYBOARD_IRQ} (vector {vector})");
    }

    /// Whether a PS/2 keyboard was set up.
    pub fn keyboard_present() -> bool {
        PRESENT.load(Ordering::Relaxed)
    }
}
