// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The boundary with the C half (P14 K13-K14, `docs/bsdkpi.md`): the
//! `bsdkpi_host_*` functions the shim (`shim/bsdkpi.c`) calls, and safe
//! wrappers for the `bsdkpi_em_*` functions it gives. The only module of
//! bsdnet with `unsafe` code (as `classic/src/ffi.rs`).
//!
//! Everything runs in the program's one thread; the host side keeps the
//! device, its mappings and the network tap in [`HOST`].

#![allow(unsafe_code)]

use alloc::alloc::{Layout, alloc, dealloc};
use alloc::vec::Vec;
use libstela::device::{Bar, Device, NetTap};
use libstela::ipc::Mapping;
use libstela::sync::SpinMutex;
use libstela::{print, println};

/// What the C half reaches through the host functions.
pub struct Host {
    /// The PCI function.
    pub device: Device,
    /// Mapped BARs (kept for the program's life).
    pub bars: Vec<Bar>,
    /// DMA memory (kept for the program's life).
    pub dma: Vec<Mapping>,
    /// The kernel's side of the network card, once attached.
    pub tap: Option<NetTap>,
    /// Frames received before the tap exists.
    pub received: u64,
}

/// The host state (set once at start).
pub static HOST: SpinMutex<Option<Host>> = SpinMutex::new(None);

unsafe extern "C" {
    fn bsdkpi_em_attach(
        vendor: u16,
        device: u16,
        subvendor: u16,
        subdevice: u16,
        revid: u8,
        mac: *mut u8,
    ) -> i32;
    fn bsdkpi_em_interrupt();
    fn bsdkpi_em_tick();
    fn bsdkpi_em_transmit(data: *const u8, len: usize) -> i32;
}

/// Attaches if_em to the device in [`HOST`]; its MAC address, or the
/// FreeBSD errno.
pub fn attach(ids: (u16, u16, u16, u16, u8)) -> Result<[u8; 6], i32> {
    let mut mac = [0u8; 6];
    // SAFETY: the C half only writes six bytes to `mac`; it runs in this
    // thread and calls back only the host functions below, none of which
    // is running now.
    let r = unsafe { bsdkpi_em_attach(ids.0, ids.1, ids.2, ids.3, ids.4, mac.as_mut_ptr()) };
    if r == 0 { Ok(mac) } else { Err(r) }
}

/// An interrupt (or a poll).
pub fn interrupt() {
    // SAFETY: as in `attach`; after a successful attach.
    unsafe { bsdkpi_em_interrupt() }
}

/// Callouts and iflib's timer.
pub fn tick() {
    // SAFETY: as in `attach`.
    unsafe { bsdkpi_em_tick() }
}

/// Sends a frame; false when the ring is full.
pub fn transmit(frame: &[u8]) -> bool {
    // SAFETY: the C half reads `frame.len()` bytes from the pointer.
    unsafe { bsdkpi_em_transmit(frame.as_ptr(), frame.len()) == 0 }
}

/// `bsdkpi_host_print`: a line from the driver.
#[unsafe(no_mangle)]
pub extern "C" fn bsdkpi_host_print(s: *const u8, n: usize) {
    if s.is_null() {
        return;
    }
    // SAFETY: the C half passes a buffer of `n` bytes it owns.
    let bytes = unsafe { core::slice::from_raw_parts(s, n) };
    print!("{}", alloc::string::String::from_utf8_lossy(bytes));
}

/// `bsdkpi_host_alloc`: heap memory (16-byte aligned).
#[unsafe(no_mangle)]
pub extern "C" fn bsdkpi_host_alloc(n: usize) -> *mut u8 {
    match Layout::from_size_align(n.max(1), 16) {
        // SAFETY: the layout has a nonzero size.
        Ok(l) => unsafe { alloc(l) },
        Err(_) => core::ptr::null_mut(),
    }
}

/// `bsdkpi_host_free`: what `bsdkpi_host_alloc` gave, with its size.
///
/// # Safety
/// `p` came from `bsdkpi_host_alloc(n)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsdkpi_host_free(p: *mut u8, n: usize) {
    if let Ok(l) = Layout::from_size_align(n.max(1), 16) {
        // SAFETY: the caller's promise.
        unsafe { dealloc(p, l) }
    }
}

/// `bsdkpi_host_dma`: zeroed memory the device reaches; its physical
/// address in `*phys`.
///
/// # Safety
/// `phys` points to a u64 the caller owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsdkpi_host_dma(bytes: usize, phys: *mut u64) -> *mut u8 {
    let mut host = HOST.lock();
    let Some(h) = host.as_mut() else {
        return core::ptr::null_mut();
    };
    let Ok((map, p)) = h.device.dma(bytes.div_ceil(4096).max(1)) else {
        println!("bsdnet: no DMA memory for {bytes} bytes");
        return core::ptr::null_mut();
    };
    let addr = map.addr();
    h.dma.push(map);
    // SAFETY: the caller's promise.
    unsafe { *phys = p };
    addr as *mut u8
}

/// `bsdkpi_host_now_us`: microseconds since boot.
#[unsafe(no_mangle)]
pub extern "C" fn bsdkpi_host_now_us() -> u64 {
    libstela::now_ns() / 1000
}

/// `bsdkpi_host_delay_us`: waits (sleeps from a millisecond up, spins
/// below).
#[unsafe(no_mangle)]
pub extern "C" fn bsdkpi_host_delay_us(us: u64) {
    if us >= 1000 {
        libstela::sleep_ms(us.div_ceil(1000));
    } else {
        let end = libstela::now_ns() + us * 1000;
        while libstela::now_ns() < end {
            core::hint::spin_loop();
        }
    }
}

/// `bsdkpi_host_pci_read`: configuration space.
#[unsafe(no_mangle)]
pub extern "C" fn bsdkpi_host_pci_read(reg: i32, width: i32) -> u32 {
    HOST.lock()
        .as_ref()
        .and_then(|h| h.device.config_read(reg as u16, width as u8).ok())
        .unwrap_or(0xFFFF_FFFF)
}

/// `bsdkpi_host_pci_write`.
#[unsafe(no_mangle)]
pub extern "C" fn bsdkpi_host_pci_write(reg: i32, val: u32, width: i32) {
    if let Some(h) = HOST.lock().as_ref() {
        let _ = h.device.config_write(reg as u16, width as u8, val);
    }
}

/// `bsdkpi_host_bar`: memory BAR `index` mapped (uncached); its size in
/// `*size`.
///
/// # Safety
/// `size` points to a u64 the caller owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsdkpi_host_bar(index: i32, size: *mut u64) -> *mut u8 {
    let mut host = HOST.lock();
    let Some(h) = host.as_mut() else {
        return core::ptr::null_mut();
    };
    let Ok(bar) = h.device.map_bar(index as u8) else {
        return core::ptr::null_mut();
    };
    let base = bar.base();
    // SAFETY: the caller's promise.
    unsafe { *size = bar.size() };
    h.bars.push(bar);
    base as *mut u8
}

/// `bsdkpi_host_input`: a received frame for the kernel.
#[unsafe(no_mangle)]
pub extern "C" fn bsdkpi_host_input(frame: *const u8, len: usize) {
    // SAFETY: the C half passes a buffer of `len` bytes.
    let bytes = unsafe { core::slice::from_raw_parts(frame, len) };
    let mut host = HOST.lock();
    if let Some(h) = host.as_mut() {
        h.received += 1;
        if let Some(tap) = &h.tap {
            let _ = tap.deliver(bytes);
        }
    }
}

/// `bsdkpi_host_link`: the link went up or down.
#[unsafe(no_mangle)]
pub extern "C" fn bsdkpi_host_link(up: i32, baudrate: u64) {
    if up != 0 {
        println!("bsdnet: em0: link up, {} Mbps", baudrate / 1_000_000);
    } else {
        println!("bsdnet: em0: link down");
    }
}
