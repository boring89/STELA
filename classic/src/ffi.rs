// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The boundary to the C code: declarations of the C functions and safe
//! wrappers around them. Every `unsafe` block of the Classic environment
//! is in this file (`CLAUDE.md`, the unsafe rule).
//!
//! The C side keeps its state in globals (Musashi has one CPU, `vm.c` one
//! bus, `c/shim/host.c` one Macintosh), so there is at most one machine at
//! a time, a bare [`Vm`] or a [`Mac`]: creating one fails while another
//! exists.

#![allow(unsafe_code)]

use alloc::boxed::Box;
use alloc::vec;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::sync::atomic::{AtomicBool, Ordering};

/// The CPU models `m68k_vm_init` offers (`c/m68k/vm.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Cpu {
    /// MC68000.
    M68000 = 0,
    /// MC68030 (the Mac IIci's), with its PMMU.
    M68030 = 1,
}

/// Register numbers of `m68k_vm_get_reg` (`c/m68k/vm.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg {
    /// Data register 0-7.
    D(u8),
    /// Address register 0-7 (A7 is the active stack pointer).
    A(u8),
    /// Program counter.
    Pc,
    /// Status register.
    Sr,
}

impl Reg {
    fn raw(self) -> u32 {
        match self {
            Reg::D(n) => u32::from(n & 7),
            Reg::A(n) => 8 + u32::from(n & 7),
            Reg::Pc => 16,
            Reg::Sr => 17,
        }
    }
}

/// An A-line trap handler: the full opcode and the address of the opcode.
pub type TrapHandler = extern "C" fn(opcode: u16, pc: u32);

/// `m68k_trap_record_t`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TrapRecord {
    opcode: u16,
    pc: u32,
}

/// `classic_host_ops` (`c/shim/host.h`).
#[repr(C)]
struct HostOps {
    disk_read: extern "C" fn(ctx: *mut c_void, offset: u32, buf: *mut u8, len: u32) -> c_int,
    disk_write: extern "C" fn(ctx: *mut c_void, offset: u32, buf: *const u8, len: u32) -> c_int,
    ctx: *mut c_void,
    disk_blocks: u32,
    disk_read_only: bool,
}

unsafe extern "C" {
    fn classic_start(
        rom: *mut u8,
        rom_size: u32,
        ram: *mut u8,
        ram_size: u32,
        vram: *mut u8,
        ops: *const HostOps,
        xpram: *const u8,
        rtc_seconds: u32,
    ) -> c_int;
    fn classic_frame() -> c_int;
    fn classic_screen_size(width: *mut u32, height: *mut u32);
    fn classic_render(out: *mut u32) -> c_int;
    fn classic_mouse(x: c_int, y: c_int, button: bool);
    fn classic_key(adb_code: u8, down: bool);
    fn classic_xpram(out: *mut u8) -> bool;
    fn classic_milestones() -> u32;
    fn m68k_vm_init(ram: *mut u8, size: u32, cpu: u32) -> c_int;
    fn m68k_vm_load(addr: u32, data: *const u8, len: u32) -> c_int;
    fn m68k_vm_reset(entry: u32, sp: u32) -> c_int;
    fn m68k_vm_run(cycles: c_int) -> c_int;
    fn m68k_vm_halted() -> bool;
    fn m68k_vm_get_reg(reg: u32) -> u32;
    fn m68k_vm_set_reg(reg: u32, value: u32);
    fn m68k_vm_read8(addr: u32) -> u32;
    fn m68k_vm_read16(addr: u32) -> u32;
    fn m68k_vm_read32(addr: u32) -> u32;
    fn m68k_vm_write8(addr: u32, value: u32);
    fn m68k_vm_write16(addr: u32, value: u32);
    fn m68k_vm_write32(addr: u32, value: u32);
    fn m68k_trap_init();
    fn m68k_trap_set_handler(number: u16, handler: Option<TrapHandler>) -> c_int;
    fn m68k_trap_dispatch(opcode: u16, pc: u32);
    fn m68k_trap_total() -> u32;
    fn m68k_trap_log_length() -> u32;
    fn m68k_trap_log_get(index: u32, out: *mut TrapRecord) -> c_int;
}

/// Whether a [`Vm`] exists.
static VM_IN_USE: AtomicBool = AtomicBool::new(false);

/// The bare 68k machine of `c/m68k/vm.c`: Musashi with flat RAM, used by
/// the CPU tests (the Macintosh model replaces the flat bus).
pub struct Vm {
    /// Guest RAM; the C side keeps a pointer to it until the next
    /// `m68k_vm_init`, so it lives as long as the Vm.
    ram: Box<[u8]>,
}

impl Vm {
    /// A machine with `ram_size` bytes of zeroed RAM; `None` if another
    /// Vm exists or the C side refuses the arguments (fewer than 8 bytes).
    pub fn new(ram_size: u32, cpu: Cpu) -> Option<Vm> {
        if VM_IN_USE.swap(true, Ordering::Acquire) {
            return None;
        }
        let mut ram = vec![0u8; ram_size as usize].into_boxed_slice();
        // SAFETY: `ram` is a live allocation of `ram_size` bytes that the
        // Vm keeps (and never moves: it is boxed) until it is dropped, and
        // no other Vm uses the C globals (VM_IN_USE).
        let r = unsafe { m68k_vm_init(ram.as_mut_ptr(), ram_size, cpu as u32) };
        if r != 0 {
            VM_IN_USE.store(false, Ordering::Release);
            return None;
        }
        // SAFETY: plain call; clears the trap table.
        unsafe { m68k_trap_init() };
        Some(Vm { ram })
    }

    /// The guest RAM.
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// Copies `data` to guest address `addr`; `false` if it does not fit.
    pub fn load(&mut self, addr: u32, data: &[u8]) -> bool {
        let Ok(len) = u32::try_from(data.len()) else {
            return false;
        };
        // SAFETY: `data` is valid for `len` bytes; the C side copies them
        // into the RAM it was given (bounds checked there).
        unsafe { m68k_vm_load(addr, data.as_ptr(), len) == 0 }
    }

    /// Resets the CPU to supervisor mode at `entry` with stack `sp`.
    pub fn reset(&mut self, entry: u32, sp: u32) -> bool {
        // SAFETY: plain call on the initialised VM.
        unsafe { m68k_vm_reset(entry, sp) == 0 }
    }

    /// Runs for about `cycles` cycles (stops early at STOP); returns the
    /// cycles used.
    pub fn run(&mut self, cycles: i32) -> i32 {
        // SAFETY: plain call; the RAM the CPU accesses is owned by self.
        unsafe { m68k_vm_run(cycles) }
    }

    /// Whether the program has executed STOP since the reset.
    pub fn halted(&self) -> bool {
        // SAFETY: plain call.
        unsafe { m68k_vm_halted() }
    }

    /// Reads a register.
    pub fn reg(&self, reg: Reg) -> u32 {
        // SAFETY: plain call; any register number is accepted.
        unsafe { m68k_vm_get_reg(reg.raw()) }
    }

    /// Writes a register.
    pub fn set_reg(&mut self, reg: Reg, value: u32) {
        // SAFETY: plain call.
        unsafe { m68k_vm_set_reg(reg.raw(), value) }
    }

    /// Reads 1, 2 or 4 bytes (big-endian) at `addr`; 0 out of range.
    pub fn read(&self, addr: u32, size: u32) -> u32 {
        // SAFETY: the C side checks the address against the RAM size.
        unsafe {
            match size {
                1 => m68k_vm_read8(addr),
                2 => m68k_vm_read16(addr),
                _ => m68k_vm_read32(addr),
            }
        }
    }

    /// Writes 1, 2 or 4 bytes (big-endian) at `addr`; ignored out of range.
    pub fn write(&mut self, addr: u32, size: u32, value: u32) {
        // SAFETY: the C side checks the address against the RAM size.
        unsafe {
            match size {
                1 => m68k_vm_write8(addr, value),
                2 => m68k_vm_write16(addr, value),
                _ => m68k_vm_write32(addr, value),
            }
        }
    }

    /// Installs (or with `None` removes) the handler of A-line trap
    /// `number` (`0..0x1000`); `false` for a bad number.
    pub fn set_trap_handler(&mut self, number: u16, handler: Option<TrapHandler>) -> bool {
        // SAFETY: the handler is a plain `extern "C"` function pointer,
        // valid for the life of the program.
        unsafe { m68k_trap_set_handler(number, handler) == 0 }
    }

    /// Records an A-line trap as the CPU would (for tests).
    pub fn dispatch_trap(&mut self, opcode: u16, pc: u32) {
        // SAFETY: plain call.
        unsafe { m68k_trap_dispatch(opcode, pc) }
    }

    /// A-line traps dispatched since the reset of the trap table.
    pub fn trap_total(&self) -> u32 {
        // SAFETY: plain call.
        unsafe { m68k_trap_total() }
    }

    /// Entries in the trap log.
    pub fn trap_log_len(&self) -> u32 {
        // SAFETY: plain call.
        unsafe { m68k_trap_log_length() }
    }

    /// Entry `index` of the trap log (oldest first): (opcode, pc).
    pub fn trap_log(&self, index: u32) -> Option<(u16, u32)> {
        let mut record = TrapRecord::default();
        // SAFETY: `record` is a valid, writable TrapRecord.
        let r = unsafe { m68k_trap_log_get(index, &mut record) };
        (r == 0).then_some((record.opcode, record.pc))
    }
}

impl Drop for Vm {
    fn drop(&mut self) {
        VM_IN_USE.store(false, Ordering::Release);
    }
}

/// The Mac's System disk, as the Rust side provides it.
pub trait MacDisk {
    /// Size in 512-byte blocks.
    fn blocks(&self) -> u32;
    /// Whether writes are refused.
    fn read_only(&self) -> bool;
    /// Reads `buf.len()` bytes at byte `offset`; `false` on failure.
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> bool;
    /// Writes `data` at byte `offset`; `false` on failure.
    fn write(&mut self, offset: u32, data: &[u8]) -> bool;
}

/// Why [`Mac::start`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    /// Another machine exists.
    Busy,
    /// The ROM is not the Mac IIci ROM (checksum 368CADFE).
    BadRom,
    /// The model refused the sizes (the ROM must be a power of two of at
    /// least 64 KiB).
    BadArguments,
}

/// What the Mac OS asked for at its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    /// Still running.
    None,
    /// Special > Shut Down.
    Off,
    /// Special > Restart.
    Restart,
}

/// Size of the built-in video RAM (`CLASSIC_VRAM_SIZE`).
const VRAM_SIZE: usize = 0x8_0000;

extern "C" fn disk_read_trampoline(ctx: *mut c_void, offset: u32, buf: *mut u8, len: u32) -> c_int {
    // SAFETY: `ctx` is the `Box<dyn MacDisk>` that the running Mac owns
    // (set in `Mac::start`, valid until the Mac is dropped, and the model
    // only calls back from inside `Mac::frame`, which borrows the Mac
    // mutably); `buf` is valid for `len` bytes (the model's own buffer).
    let (disk, buf) = unsafe {
        (
            &mut *(ctx as *mut alloc::boxed::Box<dyn MacDisk>),
            core::slice::from_raw_parts_mut(buf, len as usize),
        )
    };
    if disk.read(offset, buf) { 0 } else { -1 }
}

extern "C" fn disk_write_trampoline(
    ctx: *mut c_void,
    offset: u32,
    buf: *const u8,
    len: u32,
) -> c_int {
    // SAFETY: as in `disk_read_trampoline`; `buf` is readable for `len`
    // bytes.
    let (disk, data) = unsafe {
        (
            &mut *(ctx as *mut alloc::boxed::Box<dyn MacDisk>),
            core::slice::from_raw_parts(buf, len as usize),
        )
    };
    if disk.write(offset, data) { 0 } else { -1 }
}

/// The Macintosh IIci of `c/mac` with the built-in video (640x480).
pub struct Mac {
    // The C side holds pointers into all of these until the Mac is
    // dropped; boxes never move their contents.
    _rom: Box<[u8]>,
    _ram: Box<[u8]>,
    _vram: Box<[u8]>,
    _disk: Box<Box<dyn MacDisk>>,
}

impl Mac {
    /// Patches and attaches the ROM, gives the Mac `ram_size` bytes of
    /// RAM, the disk, the parameter RAM and the clock (seconds since
    /// 1 January 1904), and resets it.
    pub fn start(
        rom: Box<[u8]>,
        ram_size: u32,
        disk: Box<dyn MacDisk>,
        xpram: &[u8; 256],
        rtc_seconds: u32,
    ) -> Result<Mac, StartError> {
        if VM_IN_USE.swap(true, Ordering::Acquire) {
            return Err(StartError::Busy);
        }
        let mut rom = rom;
        let mut ram = vec![0u8; ram_size as usize].into_boxed_slice();
        let mut vram = vec![0u8; VRAM_SIZE].into_boxed_slice();
        let mut disk = Box::new(disk);
        let ops = HostOps {
            disk_read: disk_read_trampoline,
            disk_write: disk_write_trampoline,
            disk_blocks: disk.blocks(),
            disk_read_only: disk.read_only(),
            ctx: &mut *disk as *mut Box<dyn MacDisk> as *mut c_void,
        };
        let Ok(rom_size) = u32::try_from(rom.len()) else {
            VM_IN_USE.store(false, Ordering::Release);
            return Err(StartError::BadArguments);
        };
        // SAFETY: the ROM, RAM and video RAM buffers have the sizes
        // passed (the video RAM CLASSIC_VRAM_SIZE) and are kept by the Mac
        // until it is dropped; `ops` is copied by the callee and its `ctx`
        // points at the boxed disk the Mac also keeps; `xpram` is 256
        // bytes; no other machine uses the C globals (VM_IN_USE).
        let r = unsafe {
            classic_start(
                rom.as_mut_ptr(),
                rom_size,
                ram.as_mut_ptr(),
                ram_size,
                vram.as_mut_ptr(),
                &ops,
                xpram.as_ptr(),
                rtc_seconds,
            )
        };
        match r {
            0 => Ok(Mac {
                _rom: rom,
                _ram: ram,
                _vram: vram,
                _disk: disk,
            }),
            e => {
                VM_IN_USE.store(false, Ordering::Release);
                Err(if e == -1 {
                    StartError::BadRom
                } else {
                    StartError::BadArguments
                })
            }
        }
    }

    /// Runs one 60 Hz frame (416,000 cycles of the 25 MHz 68030).
    pub fn frame(&mut self) -> Power {
        // SAFETY: the machine was started; the disk callbacks it makes
        // reach the disk this Mac owns (borrowed mutably through self).
        match unsafe { classic_frame() } {
            1 => Power::Off,
            2 => Power::Restart,
            _ => Power::None,
        }
    }

    /// The screen size.
    pub fn screen_size(&self) -> (u32, u32) {
        let (mut w, mut h) = (0, 0);
        // SAFETY: both pointers are valid u32s.
        unsafe { classic_screen_size(&mut w, &mut h) };
        (w, h)
    }

    /// Draws the screen into `out` (0x00RRGGBB, `width * height` of
    /// [`screen_size`](Self::screen_size)); `false` if `out` is too small
    /// or there is no picture.
    pub fn render(&mut self, out: &mut [u32]) -> bool {
        let (w, h) = self.screen_size();
        if out.len() < (w as usize) * (h as usize) {
            return false;
        }
        // SAFETY: `out` holds at least width * height pixels, what the
        // model writes.
        unsafe { classic_render(out.as_mut_ptr()) >= 0 }
    }

    /// Puts the pointer at (x, y) of the Mac's screen with the button
    /// state.
    pub fn mouse(&mut self, x: i32, y: i32, button: bool) {
        // SAFETY: plain call.
        unsafe { classic_mouse(x, y, button) }
    }

    /// An ADB key code (`keys::adb_code`) goes down or up.
    pub fn key(&mut self, adb_code: u8, down: bool) {
        // SAFETY: plain call; the model queues the packet.
        unsafe { classic_key(adb_code, down) }
    }

    /// The parameter RAM, and whether the Mac OS changed it since the
    /// last call.
    pub fn xpram(&mut self) -> ([u8; 256], bool) {
        let mut out = [0u8; 256];
        // SAFETY: `out` is 256 writable bytes.
        let dirty = unsafe { classic_xpram(out.as_mut_ptr()) };
        (out, dirty)
    }

    /// Whether the Finder runs (low memory CurApName).
    pub fn finder_running(&self) -> bool {
        // SAFETY: plain call; reads guest RAM through the model's bus.
        unsafe { classic_milestones() & 1 != 0 }
    }
}

impl Drop for Mac {
    fn drop(&mut self) {
        VM_IN_USE.store(false, Ordering::Release);
    }
}

/// Called by the C side on an error it cannot recover from (Musashi's
/// `fatalerror`, the stand-in libc's unsupported functions).
///
/// # Safety
///
/// `message` must be a NUL-terminated string (the C callers pass string
/// literals).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn m68k_vm_fatal(message: *const c_char) -> ! {
    // SAFETY: the caller passes a NUL-terminated string.
    let text = unsafe { CStr::from_ptr(message) };
    panic!("classic: {}", text.to_str().unwrap_or("fatal error"));
}
