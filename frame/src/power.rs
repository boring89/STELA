// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning the machine off and restarting it.
//!
//! Soft off (ACPI Specification 6.5, 4.8.3.2.1 "PM1 Control Registers" and
//! 16.1.7 "Transitioning from the Working to the Sleeping State"): write
//! SLP_TYPx (from the DSDT's `\_S5`, see `acpi::aml`) into bits 12:10 of
//! the PM1a control register (and PM1b, if there is one) together with
//! SLP_EN, bit 13. The machine must be in ACPI mode (PM1 control bit 0,
//! SCI_EN); if the firmware left it in legacy mode, writing ACPI_ENABLE to
//! the SMI command port switches it.
//!
//! Restart, in order, until one works:
//!
//! 1. the FADT's reset register (an I/O port or a byte in memory), written
//!    with RESET_VALUE (ACPI 6.5, 4.8.3.6 "Reset Register");
//! 2. the 8042 keyboard controller's command 0xFE, which pulses the CPU
//!    reset line (OSDev Wiki "Reboot");
//! 3. a triple fault: an empty IDT and an exception that cannot be
//!    delivered (Intel SDM Vol. 3A 6.15 "Double Fault Exception": a fault
//!    while delivering a double fault shuts the processor down, and the
//!    chipset then resets it).

use crate::arch::x86_64::io::{inb, inw, outb, outw};
use crate::{acpi, mm};
use core::arch::asm;

/// PM1 control bit 0: the machine is in ACPI mode.
const SCI_EN: u16 = 1 << 0;
/// PM1 control bits 12:10.
const SLP_TYP_SHIFT: u16 = 10;
const SLP_TYP_MASK: u16 = 7 << SLP_TYP_SHIFT;
/// PM1 control bit 13: enter the sleep state written in SLP_TYP.
const SLP_EN: u16 = 1 << 13;

/// The 8042's status and command port; status bit 1 = input buffer full.
const I8042_COMMAND: u16 = 0x64;
const I8042_INPUT_FULL: u8 = 1 << 1;
/// 8042 command: pulse the reset line.
const I8042_RESET: u8 = 0xFE;

/// Port reads to wait for something to happen (a read takes about a
/// microsecond, so this is about a second; the timer does not run with
/// interrupts off).
const SPIN_READS: u32 = 1_000_000;

/// Why the machine could not be turned off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerError {
    /// No FADT PM1a control block or no `\_S5` object.
    NotSupported,
    /// The registers were written but the machine kept running.
    Failed,
}

/// Burns about `reads` microseconds by reading an unused port (0x80, the
/// POST code port, is the traditional one for delays).
fn spin(reads: u32) {
    for _ in 0..reads {
        // SAFETY: reading port 0x80 has no effect on any device.
        let _ = unsafe { inb(0x80) };
    }
}

/// The PM1 control ports and the S5 sleep types, if ACPI soft off is
/// possible on this machine.
fn soft_off_registers() -> Option<(acpi::fadt::Fadt, (u8, u8))> {
    let fadt = *acpi::fadt()?;
    let s5 = acpi::s5()?;
    (fadt.pm1a_control != 0 && fadt.pm1a_control <= 0xFFFF).then_some((fadt, s5))
}

/// Whether [`shutdown`] can work: the FADT has a PM1a control block and
/// the DSDT a `\_S5` object.
pub fn can_shutdown() -> bool {
    soft_off_registers().is_some()
}

/// Writes SLP_TYP | SLP_EN into a PM1 control register, keeping its other
/// bits.
fn enter_sleep(port: u16, sleep_type: u8) {
    // SAFETY: the port is the FADT's PM1 control block, which belongs to
    // this module; the write changes the power state only.
    unsafe {
        let value = inw(port) & !(SLP_TYP_MASK | SLP_EN);
        outw(
            port,
            value | (u16::from(sleep_type & 7) << SLP_TYP_SHIFT) | SLP_EN,
        );
    }
}

/// Turns the machine off (ACPI S5). Interrupts are disabled first. Returns
/// only if it did not work. The caller flushes the file systems before.
pub fn shutdown() -> PowerError {
    let Some((fadt, (type_a, type_b))) = soft_off_registers() else {
        return PowerError::NotSupported;
    };
    crate::kprintln!("power: turning off");
    let _irq = crate::sync::IrqGuard::new();
    let pm1a = fadt.pm1a_control as u16;
    // SAFETY: the PM1a control port and the SMI command port come from the
    // FADT; reading PM1a and writing ACPI_ENABLE to the SMI port only
    // switches the firmware to ACPI mode.
    unsafe {
        if inw(pm1a) & SCI_EN == 0 && fadt.smi_command != 0 && fadt.acpi_enable != 0 {
            outb(fadt.smi_command as u16, fadt.acpi_enable);
            for _ in 0..3000 {
                if inw(pm1a) & SCI_EN != 0 {
                    break;
                }
                spin(1000);
            }
        }
    }
    enter_sleep(pm1a, type_a);
    if fadt.pm1b_control != 0 && fadt.pm1b_control <= 0xFFFF {
        enter_sleep(fadt.pm1b_control as u16, type_b);
    }
    spin(SPIN_READS);
    crate::kprintln!("power: the machine is still running after the S5 request");
    PowerError::Failed
}

/// Restarts the machine. Interrupts are disabled first. The caller
/// flushes the file systems before.
pub fn reboot() -> ! {
    crate::kprintln!("power: restarting");
    let _irq = crate::sync::IrqGuard::new();
    if let Some(reset) = acpi::fadt().and_then(|f| f.reset) {
        match reset.space {
            acpi::fadt::SPACE_IO if reset.address <= 0xFFFF => {
                // SAFETY: the FADT names this port as the reset register;
                // writing RESET_VALUE resets the machine, nothing else.
                unsafe { outb(reset.address as u16, reset.value) };
            }
            acpi::fadt::SPACE_MEMORY => {
                if let Ok(region) = mm::mmio::map(reset.address, 1) {
                    region.write_u8(0, reset.value);
                }
            }
            _ => {}
        }
        spin(SPIN_READS / 2);
    }
    if acpi::fadt().is_none_or(|f| f.has_8042) {
        // SAFETY: the 8042's command port; waiting for its input buffer
        // and sending 0xFE only pulses the reset line. The keyboard driver
        // is not running (interrupts are off).
        unsafe {
            for _ in 0..100_000 {
                if inb(I8042_COMMAND) & I8042_INPUT_FULL == 0 {
                    break;
                }
            }
            outb(I8042_COMMAND, I8042_RESET);
        }
        spin(SPIN_READS / 2);
    }
    triple_fault()
}

/// Loads an empty IDT and raises an exception: it cannot be delivered,
/// nor can the double fault, so the processor shuts down and the chipset
/// resets it.
fn triple_fault() -> ! {
    #[repr(C, packed)]
    struct Pointer {
        limit: u16,
        base: u64,
    }
    let empty = Pointer { limit: 0, base: 0 };
    // SAFETY: the machine is meant to reset; nothing runs after this. With
    // a zero-limit IDT every interrupt and exception escalates to a
    // shutdown, which never returns.
    unsafe {
        asm!("lidt [{}]", "int3", in(reg) &empty, options(readonly, nostack));
    }
    crate::halt()
}
