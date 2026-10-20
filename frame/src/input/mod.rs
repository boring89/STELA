// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Input events: one queue for every input device.
//!
//! ```text
//!  IRQ 1 ──► i8042 byte ──► Decoder ──► KeyEvent ──┐
//!  IRQ 12 ─► i8042 byte ──► MouseDecoder ► Mouse ───┤
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
pub mod mouse;
pub mod queue;

pub use abi::key::{KeyCode, KeyEvent, Modifiers, MouseEvent, key_to_char};

use crate::sync::{Once, SpinLock};
use queue::EventQueue;

/// Something the user did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// A key was pressed or released.
    Key(KeyEvent),
    /// The mouse moved or a button changed.
    Mouse(MouseEvent),
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
pub use target::{init, keyboard_present, mouse_present};

#[cfg(target_os = "none")]
mod target {
    use super::keyboard::Decoder;
    use super::mouse::MouseDecoder;
    use super::{InputEvent, push};
    use crate::arch::x86_64::i8042;
    use crate::irq;
    use crate::sync::SpinLock;
    use crate::trap::TrapFrame;
    use core::sync::atomic::{AtomicBool, Ordering};

    static DECODER: SpinLock<Decoder> = SpinLock::new(Decoder::new());
    static MOUSE: SpinLock<MouseDecoder> = SpinLock::new(MouseDecoder::new());
    static PRESENT: AtomicBool = AtomicBool::new(false);
    static MOUSE_PRESENT: AtomicBool = AtomicBool::new(false);

    /// The legacy IRQ of the PS/2 keyboard.
    const KEYBOARD_IRQ: u8 = 1;
    /// The legacy IRQ of the PS/2 mouse (the i8042's second port).
    const MOUSE_IRQ: u8 = 12;

    /// Takes every waiting byte, from either port, to its decoder. Both
    /// interrupts run this: the two ports share one output buffer.
    fn drain(_: &TrapFrame) {
        while let Some((byte, from_mouse)) = i8042::read_byte() {
            if from_mouse {
                let event = MOUSE.lock().feed(byte);
                if let Some(event) = event {
                    push(InputEvent::Mouse(event));
                }
            } else {
                let event = DECODER.lock().feed(byte);
                if let Some(event) = event {
                    push(InputEvent::Key(event));
                }
            }
        }
    }

    /// Routes `irq` to [`drain`].
    fn route(irq: u8) -> Result<u8, irq::IrqError> {
        let vector = irq::isa_vector(irq);
        irq::register(vector, drain)?;
        if let Err(e) = irq::enable_isa(irq) {
            irq::unregister(vector);
            return Err(e);
        }
        Ok(vector)
    }

    /// Sets up the PS/2 keyboard and routes IRQ 1 to it. A machine without
    /// an i8042 (many new PCs) just has no keyboard for now.
    pub fn init() {
        if let Err(why) = i8042::init() {
            crate::kprintln!("i8042: no PS/2 keyboard ({why})");
            return;
        }
        let vector = match route(KEYBOARD_IRQ) {
            Ok(vector) => vector,
            Err(e) => {
                crate::kprintln!("i8042: cannot route IRQ {KEYBOARD_IRQ}: {e:?}");
                return;
            }
        };
        let mouse = match i8042::init_mouse() {
            Ok(()) => match route(MOUSE_IRQ) {
                Ok(vector) => Some(vector),
                Err(e) => {
                    crate::kprintln!("i8042: cannot route IRQ {MOUSE_IRQ}: {e:?}");
                    None
                }
            },
            Err(why) => {
                crate::kprintln!("i8042: no PS/2 mouse ({why})");
                None
            }
        };
        // IRQs 1 and 12 are edge triggered: a byte that arrived before the
        // route was set up raised its edge into a masked input, and while
        // it sits in the output buffer the controller raises no new edge.
        // Drain it.
        let mut stale = 0;
        while i8042::read_byte().is_some() {
            stale += 1;
        }
        if stale > 0 {
            crate::kprintln!("i8042: discarded {stale} stale byte(s)");
        }
        PRESENT.store(true, Ordering::Relaxed);
        crate::kprintln!("i8042: PS/2 keyboard on IRQ {KEYBOARD_IRQ} (vector {vector})");
        if let Some(vector) = mouse {
            MOUSE_PRESENT.store(true, Ordering::Relaxed);
            crate::kprintln!("i8042: PS/2 mouse on IRQ {MOUSE_IRQ} (vector {vector})");
        }
    }

    /// Whether a PS/2 mouse was set up.
    pub fn mouse_present() -> bool {
        MOUSE_PRESENT.load(Ordering::Relaxed)
    }

    /// Whether a PS/2 keyboard was set up.
    pub fn keyboard_present() -> bool {
        PRESENT.load(Ordering::Relaxed)
    }
}
