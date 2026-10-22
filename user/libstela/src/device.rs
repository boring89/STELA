// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! User-space drivers (P13 K11, `docs/devices.md`): a PCI function lent by
//! the kernel ([`Device`]), its registers ([`Bar`]) and a network card
//! attached to the kernel's network ([`NetTap`]). Needs the `DEVICES`
//! capability.
//!
//! [`Bar`] is the one place a driver touches the device directly: volatile
//! 32-bit reads and writes, bounds-checked, into the BAR the kernel mapped
//! (the `unsafe` lives here, like the rest of libstela's).

use crate::ipc::{Mapping, handle_close};
use crate::sys::{syscall1, syscall2, syscall3, syscall4};
use abi::{Error, Handle, Syscall, capability};

/// A PCI function lent to this program (closing it resets the device).
pub struct Device(Handle);

impl Drop for Device {
    fn drop(&mut self) {
        let _ = handle_close(self.0);
    }
}

/// The program's `DEVICES` capability.
fn devices() -> Result<Handle, Error> {
    crate::process::capability(capability::DEVICES).ok_or(Error::AccessDenied)
}

impl Device {
    /// Opens the function at `location` (`BB:DD.F`).
    pub fn open(location: &str) -> Result<Device, Error> {
        let cap = devices()?;
        syscall3(
            Syscall::DeviceOpen,
            location.as_ptr() as u64,
            location.len() as u64,
            cap.raw(),
        )
        .map(|h| Device(Handle::from_raw(h)))
    }

    /// Reads `size` (1, 2, 4) bytes of configuration space at `offset`.
    pub fn config_read(&self, offset: u16, size: u8) -> Result<u32, Error> {
        syscall4(
            Syscall::DeviceConfig,
            self.0.raw(),
            u64::from(offset),
            u64::from(size),
            0,
        )
        .map(|v| v as u32)
    }

    /// Writes `size` bytes of configuration space at `offset`.
    pub fn config_write(&self, offset: u16, size: u8, value: u32) -> Result<(), Error> {
        syscall4(
            Syscall::DeviceConfig,
            self.0.raw(),
            u64::from(offset),
            u64::from(size) | abi::device::WRITE,
            u64::from(value),
        )
        .map(|_| ())
    }

    /// Maps memory BAR `index`.
    pub fn map_bar(&self, index: u8) -> Result<Bar, Error> {
        let mut size = 0u64;
        let base = syscall3(
            Syscall::DeviceMapBar,
            self.0.raw(),
            u64::from(index),
            &mut size as *mut u64 as u64,
        )?;
        Ok(Bar { base, size })
    }

    /// `pages` pages of contiguous memory the device can reach: mapped
    /// here, and its physical address (for descriptors).
    pub fn dma(&self, pages: usize) -> Result<(Mapping, u64), Error> {
        let mut phys = 0u64;
        let h = syscall3(
            Syscall::DeviceDma,
            self.0.raw(),
            pages as u64,
            &mut phys as *mut u64 as u64,
        )?;
        let memory = Handle::from_raw(h);
        let map = Mapping::new(memory, pages as u64 * 4096, true);
        let _ = handle_close(memory);
        Ok((map?, phys))
    }

    /// Sends the device's interrupts (MSI) to `port` as `DEVICE_IRQ`
    /// messages; the vector.
    pub fn irq(&self, port: Handle) -> Result<u8, Error> {
        syscall2(Syscall::DeviceIrq, self.0.raw(), port.raw()).map(|v| v as u8)
    }

    /// Bus mastering and MSI off.
    pub fn reset(&self) -> Result<(), Error> {
        syscall1(Syscall::DeviceReset, self.0.raw()).map(|_| ())
    }
}

/// A memory BAR mapped into this program.
pub struct Bar {
    base: u64,
    size: u64,
}

impl Bar {
    /// Its size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    fn check(&self, offset: usize) -> bool {
        offset.is_multiple_of(4) && (offset as u64) + 4 <= self.size
    }

    /// Reads the 32-bit register at `offset` (0 outside the BAR).
    pub fn read_u32(&self, offset: usize) -> u32 {
        if !self.check(offset) {
            return 0;
        }
        // SAFETY: the kernel mapped base..base + size (uncached) for this
        // program and never unmaps it; offset is aligned and inside.
        unsafe { core::ptr::read_volatile((self.base + offset as u64) as *const u32) }
    }

    /// Writes the 32-bit register at `offset` (nothing outside the BAR).
    pub fn write_u32(&self, offset: usize, value: u32) {
        if !self.check(offset) {
            return;
        }
        // SAFETY: as in read_u32.
        unsafe { core::ptr::write_volatile((self.base + offset as u64) as *mut u32, value) }
    }
}

/// A network card in this program, attached to the kernel's network.
pub struct NetTap(Handle);

impl Drop for NetTap {
    fn drop(&mut self) {
        let _ = handle_close(self.0);
    }
}

impl NetTap {
    /// Attaches a card with `mac`; the kernel sends `NET_TX_READY` to
    /// `port` when frames wait.
    pub fn attach(port: Handle, mac: [u8; 6]) -> Result<NetTap, Error> {
        let cap = devices()?;
        syscall3(
            Syscall::NetAttach,
            port.raw(),
            mac.as_ptr() as u64,
            cap.raw(),
        )
        .map(|h| NetTap(Handle::from_raw(h)))
    }

    /// A frame the card received.
    pub fn deliver(&self, frame: &[u8]) -> Result<(), Error> {
        syscall3(
            Syscall::NetDeliver,
            self.0.raw(),
            frame.as_ptr() as u64,
            frame.len() as u64,
        )
        .map(|_| ())
    }

    /// The next frame to send into `buf`; its length (0: none).
    pub fn take(&self, buf: &mut [u8]) -> Result<usize, Error> {
        syscall3(
            Syscall::NetTake,
            self.0.raw(),
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
        .map(|n| n as usize)
    }
}
