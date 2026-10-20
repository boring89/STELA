// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The i8042 PS/2 controller, first port only (the keyboard), polled during
//! set-up and interrupt driven afterwards (OSDev Wiki "8042 PS/2
//! Controller", "Initialising the PS/2 Controller"; "PS/2 Keyboard").
//!
//! ```text
//! port  read            write
//! 0x60  data            data (to the device, or a command's argument)
//! 0x64  status          command to the controller
//!
//! status bit 0  output buffer full (a byte is waiting at 0x60)
//!        bit 1  input buffer full (do not write yet)
//!        bit 5  the waiting byte is from the second port (mouse)
//!
//! configuration byte (commands 0x20 read / 0x60 write)
//!        bit 0  first port interrupt (IRQ 1)
//!        bit 1  second port interrupt (IRQ 12)
//!        bit 4  first port clock disabled
//!        bit 5  second port clock disabled
//!        bit 6  translate to scan code set 1
//! ```

use super::io::{inb, outb};

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;
const COMMAND: u16 = 0x64;

const OUTPUT_FULL: u8 = 1 << 0;
const INPUT_FULL: u8 = 1 << 1;
const FROM_PORT2: u8 = 1 << 5;

const READ_CONFIG: u8 = 0x20;
const WRITE_CONFIG: u8 = 0x60;
const DISABLE_PORT2: u8 = 0xA7;
const SELF_TEST: u8 = 0xAA;
const TEST_PORT1: u8 = 0xAB;
const DISABLE_PORT1: u8 = 0xAD;
const ENABLE_PORT1: u8 = 0xAE;

const CONFIG_IRQ1: u8 = 1 << 0;
const CONFIG_IRQ12: u8 = 1 << 1;
const CONFIG_PORT1_OFF: u8 = 1 << 4;
const CONFIG_PORT2_OFF: u8 = 1 << 5;
const CONFIG_TRANSLATE: u8 = 1 << 6;

/// Keyboard command: enable scanning; the keyboard answers 0xFA (ACK).
const KBD_ENABLE_SCANNING: u8 = 0xF4;
const KBD_ACK: u8 = 0xFA;

/// Status polls before giving up (well over 10 ms on real hardware, where
/// each port read takes about a microsecond).
const TIMEOUT: u32 = 100_000;

fn status() -> u8 {
    // SAFETY: the i8042 ports belong to this module; reading the status
    // register has no side effects.
    unsafe { inb(STATUS) }
}

/// Waits until the controller accepts a byte.
fn wait_writable() -> Result<(), &'static str> {
    for _ in 0..TIMEOUT {
        if status() & INPUT_FULL == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("controller does not accept input")
}

/// Waits for a byte and reads it.
fn read() -> Result<u8, &'static str> {
    for _ in 0..TIMEOUT {
        if status() & OUTPUT_FULL != 0 {
            // SAFETY: a byte is waiting; reading port 0x60 takes it.
            return Ok(unsafe { inb(DATA) });
        }
        core::hint::spin_loop();
    }
    Err("no reply")
}

fn command(byte: u8) -> Result<(), &'static str> {
    wait_writable()?;
    // SAFETY: a documented controller command (see the module table).
    unsafe { outb(COMMAND, byte) };
    Ok(())
}

fn write_data(byte: u8) -> Result<(), &'static str> {
    wait_writable()?;
    // SAFETY: a command argument or a byte for the keyboard.
    unsafe { outb(DATA, byte) };
    Ok(())
}

/// Throws away whatever is waiting in the output buffer.
fn flush() {
    for _ in 0..64 {
        if status() & OUTPUT_FULL == 0 {
            break;
        }
        // SAFETY: discarding a waiting byte.
        let _ = unsafe { inb(DATA) };
    }
}

/// Sets up the controller with the keyboard port enabled, scan code
/// translation (set 1) on and its interrupt (IRQ 1) on; the second port
/// stays off. Fails if there is no working controller.
pub(crate) fn init() -> Result<(), &'static str> {
    // A floating bus reads 0xFF: no controller.
    if status() == 0xFF {
        return Err("no controller");
    }
    command(DISABLE_PORT1)?;
    command(DISABLE_PORT2)?;
    flush();
    command(READ_CONFIG)?;
    let mut config = read()?;
    config &= !(CONFIG_IRQ1 | CONFIG_IRQ12);
    command(WRITE_CONFIG)?;
    write_data(config)?;
    command(SELF_TEST)?;
    if read()? != 0x55 {
        return Err("controller self test failed");
    }
    // The self test can reset the controller: write the configuration again.
    command(WRITE_CONFIG)?;
    write_data(config)?;
    command(TEST_PORT1)?;
    if read()? != 0x00 {
        return Err("keyboard port test failed");
    }
    command(ENABLE_PORT1)?;
    flush();
    // Scanning is normally on after power-up; make sure.
    write_data(KBD_ENABLE_SCANNING)?;
    if read()? != KBD_ACK {
        return Err("keyboard did not acknowledge");
    }
    // `config` was read while port 1 was disabled, so its clock-off bit is
    // set: clear it, or this write would disable the keyboard again.
    config |= CONFIG_IRQ1 | CONFIG_TRANSLATE | CONFIG_PORT2_OFF;
    config &= !(CONFIG_IRQ12 | CONFIG_PORT1_OFF);
    command(WRITE_CONFIG)?;
    write_data(config)?;
    Ok(())
}

/// The next byte from the keyboard, if one is waiting. Bytes from the
/// second port are discarded.
pub(crate) fn read_keyboard_byte() -> Option<u8> {
    loop {
        let status = status();
        if status & OUTPUT_FULL == 0 {
            return None;
        }
        // SAFETY: a byte is waiting; reading port 0x60 takes it.
        let byte = unsafe { inb(DATA) };
        if status & FROM_PORT2 == 0 {
            return Some(byte);
        }
    }
}
