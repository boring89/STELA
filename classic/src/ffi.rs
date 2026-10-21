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
    audio: extern "C" fn(ctx: *mut c_void, samples: *const u8, count: u32, rate: u32),
    audio_ctx: *mut c_void,
    fs: *const FsOps,
    card_vram: *mut u8,
    card_res: u32,
    card_depth: u32,
    more_disks: [RawDisk; MORE_DISKS],
}

/// One of `classic_host_ops.more_disks`.
#[repr(C)]
#[derive(Clone, Copy)]
struct RawDisk {
    ctx: *mut c_void,
    blocks: u32,
    read_only: bool,
}

/// Disks besides the System disk (`CLASSIC_MORE_DISKS`): drives 9 to 11.
pub const MORE_DISKS: usize = 3;

/// `mac_fs_item_t` (`c/mac/fsm.h`).
#[repr(C)]
#[derive(Clone, Copy)]
struct RawItem {
    id: u32,
    parent: u32,
    is_dir: bool,
    name: [u8; 32],
    data_length: u32,
    rsrc_length: u32,
    valence: u32,
    finfo: [u8; 16],
    fxinfo: [u8; 16],
    created: u32,
    modified: u32,
    backed_up: u32,
    locked: bool,
}

/// `mac_fs_volume_t`.
#[repr(C)]
struct RawVolume {
    total_bytes: u64,
    free_bytes: u64,
    files: u32,
    folders: u32,
    read_only: bool,
}

/// `mac_fs_ops_t`: every callback reaches the boxed [`MacFs`] in `ctx`.
#[repr(C)]
struct FsOps {
    ctx: *mut c_void,
    volume_name: *const c_char,
    get: extern "C" fn(*mut c_void, u32, *mut RawItem) -> c_int,
    lookup: extern "C" fn(*mut c_void, u32, *const u8, u32, *mut RawItem) -> c_int,
    child: extern "C" fn(*mut c_void, u32, u32, *mut RawItem) -> c_int,
    read: extern "C" fn(*mut c_void, u32, bool, u32, *mut c_void, u32) -> i32,
    volume: extern "C" fn(*mut c_void, *mut RawVolume) -> c_int,
    create: Option<extern "C" fn(*mut c_void, u32, *const u8, u32, bool, *mut RawItem) -> c_int>,
    remove: Option<extern "C" fn(*mut c_void, u32) -> c_int>,
    move_to: Option<extern "C" fn(*mut c_void, u32, u32, *const u8, u32) -> c_int>,
    set_info: Option<extern "C" fn(*mut c_void, *const RawItem) -> c_int>,
    write: Option<extern "C" fn(*mut c_void, u32, bool, u32, *const c_void, u32) -> i32>,
    set_length: Option<extern "C" fn(*mut c_void, u32, bool, u32) -> c_int>,
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
    fn classic_sysbeep() -> c_int;
    fn classic_fs_status(install: *mut i32, mounted: *mut bool, requests: *mut u32);
    fn classic_volumes(drives: *mut u16, names: *mut [u8; 28], max: c_int) -> c_int;
    fn classic_card_resolution(width: u32, height: u32) -> c_int;
    fn mac_roman_to_unicode(c: u8) -> u32;
    fn mac_roman_from_unicode(u: u32) -> c_int;
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

/// A disk of the Mac (the System disk, or another), as the Rust side
/// provides it.
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

/// The Unicode character of Mac Roman byte `c`.
pub fn mac_roman_char(c: u8) -> char {
    // SAFETY: a pure function over a table.
    let u = unsafe { mac_roman_to_unicode(c) };
    char::from_u32(u).unwrap_or('\u{FFFD}')
}

/// The Mac Roman byte of `c`, if Mac Roman has it.
pub fn mac_roman_byte(c: char) -> Option<u8> {
    // SAFETY: a pure function over a table.
    let b = unsafe { mac_roman_from_unicode(u32::from(c)) };
    u8::try_from(b).ok()
}

/// One file or folder as the Mac's File Manager sees it (`mac_fs_item_t`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FsItem {
    /// The Mac's ID (the root folder is 2, its parent 1).
    pub id: u32,
    /// The folder it is in.
    pub parent: u32,
    /// A folder.
    pub is_dir: bool,
    /// The name, Mac Roman, at most 31 bytes.
    pub name: alloc::vec::Vec<u8>,
    /// Files: the forks' lengths.
    pub data_length: u32,
    /// The resource fork's length.
    pub rsrc_length: u32,
    /// Folders: items inside.
    pub valence: u32,
    /// FInfo + FXInfo (files) or DInfo + DXInfo (folders).
    pub finder: [u8; 32],
    /// Seconds since 1904.
    pub created: u32,
    /// Seconds since 1904.
    pub modified: u32,
}

impl FsItem {
    fn to_raw(&self) -> RawItem {
        let mut name = [0u8; 32];
        let n = self.name.len().min(31);
        name[0] = n as u8;
        name[1..=n].copy_from_slice(&self.name[..n]);
        let mut finfo = [0u8; 16];
        let mut fxinfo = [0u8; 16];
        finfo.copy_from_slice(&self.finder[..16]);
        fxinfo.copy_from_slice(&self.finder[16..]);
        RawItem {
            id: self.id,
            parent: self.parent,
            is_dir: self.is_dir,
            name,
            data_length: self.data_length,
            rsrc_length: self.rsrc_length,
            valence: self.valence,
            finfo,
            fxinfo,
            created: self.created,
            modified: self.modified,
            backed_up: 0,
            locked: false,
        }
    }

    fn from_raw(raw: &RawItem) -> FsItem {
        let n = usize::from(raw.name[0]).min(31);
        let mut finder = [0u8; 32];
        finder[..16].copy_from_slice(&raw.finfo);
        finder[16..].copy_from_slice(&raw.fxinfo);
        FsItem {
            id: raw.id,
            parent: raw.parent,
            is_dir: raw.is_dir,
            name: raw.name[1..=n].to_vec(),
            data_length: raw.data_length,
            rsrc_length: raw.rsrc_length,
            valence: raw.valence,
            finder,
            created: raw.created,
            modified: raw.modified,
        }
    }
}

/// A volume's size (`mac_fs_volume_t`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FsSpace {
    /// Bytes in all.
    pub total: u64,
    /// Bytes free.
    pub free: u64,
}

/// A Mac OS error a writing call returns (`MAC_FS_..._ERR`).
pub type MacErr = i32;

/// A volume the Mac sees through the File System Manager (`c/mac/fsm.c`,
/// `docs/classic.md` §7): every call is by the Mac's IDs; names are Mac
/// Roman. Errors are Mac OS result codes (`crate::hostfs::err`).
pub trait MacFs {
    /// The item `id`.
    fn get(&mut self, id: u32) -> Option<FsItem>;
    /// The item called `name` in folder `parent` (letters without case).
    fn lookup(&mut self, parent: u32, name: &[u8]) -> Option<FsItem>;
    /// Item `index` (from 1) of folder `parent`.
    fn child(&mut self, parent: u32, index: u32) -> Option<FsItem>;
    /// Reads at `offset` of a fork; the bytes read, `None` on failure.
    fn read(&mut self, id: u32, rsrc: bool, offset: u32, buf: &mut [u8]) -> Option<usize>;
    /// The volume's size.
    fn space(&mut self) -> FsSpace;
    /// Makes a file or folder.
    fn create(&mut self, parent: u32, name: &[u8], is_dir: bool) -> Result<FsItem, MacErr>;
    /// Removes a file or an empty folder.
    fn remove(&mut self, id: u32) -> Result<(), MacErr>;
    /// Moves (and renames, with `name`) an item.
    fn move_to(&mut self, id: u32, parent: u32, name: Option<&[u8]>) -> Result<(), MacErr>;
    /// Stores the Finder information (and dates) of `item.id`.
    fn set_info(&mut self, item: &FsItem) -> Result<(), MacErr>;
    /// Writes at `offset` of a fork; the bytes written.
    fn write(&mut self, id: u32, rsrc: bool, offset: u32, data: &[u8]) -> Result<usize, MacErr>;
    /// Sets a fork's length.
    fn set_length(&mut self, id: u32, rsrc: bool, length: u32) -> Result<(), MacErr>;
}

/// The boxed file system behind `ctx`.
///
/// # Safety
///
/// `ctx` must be the `Box<Box<dyn MacFs>>` pointer `Mac::start` put in the
/// ops, while the Mac lives (the model only calls back from inside
/// `Mac::frame`, which borrows the Mac mutably).
unsafe fn fs_of<'a>(ctx: *mut c_void) -> &'a mut alloc::boxed::Box<dyn MacFs> {
    // SAFETY: the caller's contract.
    unsafe { &mut *(ctx as *mut alloc::boxed::Box<dyn MacFs>) }
}

/// Writes `item` to the model's `out`.
///
/// # Safety
///
/// `out` must be a writable `mac_fs_item_t` (the model's).
unsafe fn put_item(out: *mut RawItem, item: Option<FsItem>) -> c_int {
    match item {
        Some(i) => {
            // SAFETY: the caller's contract.
            unsafe { *out = i.to_raw() };
            0
        }
        None => -1,
    }
}

extern "C" fn fs_get(ctx: *mut c_void, id: u32, out: *mut RawItem) -> c_int {
    // SAFETY: `ctx` and `out` come from the model as documented at fs_of
    // and put_item.
    unsafe { put_item(out, fs_of(ctx).get(id)) }
}

extern "C" fn fs_lookup(
    ctx: *mut c_void,
    parent: u32,
    name: *const u8,
    len: u32,
    out: *mut RawItem,
) -> c_int {
    // SAFETY: as in fs_get; `name` is valid for `len` bytes.
    unsafe {
        let name = core::slice::from_raw_parts(name, len as usize);
        put_item(out, fs_of(ctx).lookup(parent, name))
    }
}

extern "C" fn fs_child(ctx: *mut c_void, parent: u32, index: u32, out: *mut RawItem) -> c_int {
    // SAFETY: as in fs_get.
    unsafe { put_item(out, fs_of(ctx).child(parent, index)) }
}

extern "C" fn fs_read(
    ctx: *mut c_void,
    id: u32,
    rsrc: bool,
    offset: u32,
    buf: *mut c_void,
    len: u32,
) -> i32 {
    // SAFETY: as in fs_get; `buf` is writable for `len` bytes.
    unsafe {
        let buf = core::slice::from_raw_parts_mut(buf as *mut u8, len as usize);
        match fs_of(ctx).read(id, rsrc, offset, buf) {
            Some(n) => n as i32,
            None => -1,
        }
    }
}

extern "C" fn fs_volume(ctx: *mut c_void, out: *mut RawVolume) -> c_int {
    // SAFETY: as in fs_get; `out` is a writable mac_fs_volume_t.
    unsafe {
        let s = fs_of(ctx).space();
        *out = RawVolume {
            total_bytes: s.total,
            free_bytes: s.free,
            files: 0,
            folders: 0,
            read_only: false,
        };
    }
    0
}

extern "C" fn fs_create(
    ctx: *mut c_void,
    parent: u32,
    name: *const u8,
    len: u32,
    is_dir: bool,
    out: *mut RawItem,
) -> c_int {
    // SAFETY: as in fs_lookup.
    unsafe {
        let name = core::slice::from_raw_parts(name, len as usize);
        match fs_of(ctx).create(parent, name, is_dir) {
            Ok(item) => put_item(out, Some(item)),
            Err(e) => e,
        }
    }
}

extern "C" fn fs_remove(ctx: *mut c_void, id: u32) -> c_int {
    // SAFETY: as in fs_get.
    unsafe { fs_of(ctx).remove(id).err().unwrap_or(0) }
}

extern "C" fn fs_move(ctx: *mut c_void, id: u32, parent: u32, name: *const u8, len: u32) -> c_int {
    // SAFETY: as in fs_lookup; `name` may be NULL (keep the name).
    unsafe {
        let name = (!name.is_null()).then(|| core::slice::from_raw_parts(name, len as usize));
        fs_of(ctx).move_to(id, parent, name).err().unwrap_or(0)
    }
}

extern "C" fn fs_set_info(ctx: *mut c_void, item: *const RawItem) -> c_int {
    // SAFETY: as in fs_get; `item` is a readable mac_fs_item_t.
    unsafe {
        let item = FsItem::from_raw(&*item);
        fs_of(ctx).set_info(&item).err().unwrap_or(0)
    }
}

extern "C" fn fs_write(
    ctx: *mut c_void,
    id: u32,
    rsrc: bool,
    offset: u32,
    data: *const c_void,
    len: u32,
) -> i32 {
    // SAFETY: as in fs_get; `data` is readable for `len` bytes.
    unsafe {
        let data = core::slice::from_raw_parts(data as *const u8, len as usize);
        match fs_of(ctx).write(id, rsrc, offset, data) {
            Ok(n) => n as i32,
            Err(e) => e,
        }
    }
}

extern "C" fn fs_set_length(ctx: *mut c_void, id: u32, rsrc: bool, length: u32) -> c_int {
    // SAFETY: as in fs_get.
    unsafe { fs_of(ctx).set_length(id, rsrc, length).err().unwrap_or(0) }
}

/// Size of the NuBus card's video RAM (`CLASSIC_CARD_VRAM_SIZE`).
const CARD_VRAM_SIZE: usize = 0x200_0000;

/// The NuBus video card's starting mode (P11, `docs/classic.md` §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    /// Index of the resolution in the card's list ([`card_resolution`]).
    pub resolution: u32,
    /// 0-5 = 1, 2, 4, 8, 16, 32 bits per pixel.
    pub depth: u32,
}

/// The card's index of resolution `width` x `height`, if it offers it.
pub fn card_resolution(width: u32, height: u32) -> Option<u32> {
    // SAFETY: a pure function over a table.
    u32::try_from(unsafe { classic_card_resolution(width, height) }).ok()
}

/// The depth index of `bits` per pixel (1, 2, 4, 8, 16 or 32).
pub fn card_depth(bits: u32) -> Option<u32> {
    [1, 2, 4, 8, 16, 32]
        .iter()
        .position(|&b| b == bits)
        .map(|i| i as u32)
}

/// The file system's state (`Mac::fs_status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FsStatus {
    /// The install's result: `None` before it ran, 0 or an OSErr after.
    pub install: Option<i32>,
    /// Whether the volume is on the Mac's desktop.
    pub mounted: bool,
    /// File Manager requests served.
    pub requests: u32,
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

/// The sound chip's output collected between frames (unsigned 8-bit
/// mono samples and their rate).
#[derive(Default)]
struct AudioSink {
    samples: alloc::vec::Vec<u8>,
    rate: u32,
}

/// Most samples kept between two takes (about a second); older ones are
/// dropped.
const AUDIO_KEEP: usize = 32 * 1024;

extern "C" fn audio_trampoline(ctx: *mut c_void, samples: *const u8, count: u32, rate: u32) {
    // SAFETY: `ctx` is the boxed AudioSink the running Mac owns (set in
    // `Mac::start`, called back only from inside `Mac::frame`); `samples`
    // is valid for `count` bytes (the model's own buffer).
    let (sink, data) = unsafe {
        (
            &mut *(ctx as *mut AudioSink),
            core::slice::from_raw_parts(samples, count as usize),
        )
    };
    sink.rate = rate;
    if sink.samples.len() + data.len() > AUDIO_KEEP {
        let drop = (sink.samples.len() + data.len() - AUDIO_KEEP).min(sink.samples.len());
        sink.samples.drain(..drop);
    }
    sink.samples.extend_from_slice(data);
}

/// What the C side points into for the external volume: the file
/// system, its ops and its name.
type FsParts = (Box<Box<dyn MacFs>>, Box<FsOps>, Box<[u8]>);

/// The Macintosh IIci of `c/mac` with the built-in video (640x480).
pub struct Mac {
    // The C side holds pointers into all of these until the Mac is
    // dropped; boxes never move their contents.
    _rom: Box<[u8]>,
    _ram: Box<[u8]>,
    _vram: Box<[u8]>,
    // Each disk boxed again: the C side's ctx is a thin pointer to the
    // `Box<dyn MacDisk>` (the trampolines' type), which must not move.
    #[allow(clippy::vec_box)]
    _disks: alloc::vec::Vec<Box<Box<dyn MacDisk>>>,
    audio: Box<AudioSink>,
    _fs: Option<FsParts>,
    _card_vram: Option<Box<[u8]>>,
}

impl Mac {
    /// Patches and attaches the ROM, gives the Mac `ram_size` bytes of
    /// RAM, the disks (the System disk first, drive 8; then up to
    /// [`MORE_DISKS`] more, drives 9..), the parameter RAM and the clock
    /// (seconds since 1 January 1904), and resets it.
    pub fn start(
        rom: Box<[u8]>,
        ram_size: u32,
        disks: alloc::vec::Vec<Box<dyn MacDisk>>,
        fs: Option<(Box<dyn MacFs>, &str)>,
        card: Option<Card>,
        xpram: &[u8; 256],
        rtc_seconds: u32,
    ) -> Result<Mac, StartError> {
        if VM_IN_USE.swap(true, Ordering::Acquire) {
            return Err(StartError::Busy);
        }
        if disks.is_empty() || disks.len() > 1 + MORE_DISKS {
            VM_IN_USE.store(false, Ordering::Release);
            return Err(StartError::BadArguments);
        }
        let mut rom = rom;
        let mut ram = vec![0u8; ram_size as usize].into_boxed_slice();
        let mut vram = vec![0u8; VRAM_SIZE].into_boxed_slice();
        let mut disks: alloc::vec::Vec<Box<Box<dyn MacDisk>>> =
            disks.into_iter().map(Box::new).collect();
        let mut more_disks = [RawDisk {
            ctx: core::ptr::null_mut(),
            blocks: 0,
            read_only: true,
        }; MORE_DISKS];
        for (raw, disk) in more_disks.iter_mut().zip(disks.iter_mut().skip(1)) {
            *raw = RawDisk {
                blocks: disk.blocks(),
                read_only: disk.read_only(),
                ctx: &mut **disk as *mut Box<dyn MacDisk> as *mut c_void,
            };
        }
        let disk = &mut disks[0];
        let mut audio = Box::new(AudioSink::default());
        // The external volume: its callbacks, its name (Mac Roman, NUL).
        let fs = fs.map(|(fs, name)| {
            let mut fs = Box::new(fs);
            let mut cname: alloc::vec::Vec<u8> = name
                .chars()
                .take(27)
                .map(|c| mac_roman_byte(c).filter(|&b| b != 0).unwrap_or(b'_'))
                .collect();
            cname.push(0);
            let cname = cname.into_boxed_slice();
            let ops = Box::new(FsOps {
                ctx: &mut *fs as *mut Box<dyn MacFs> as *mut c_void,
                volume_name: cname.as_ptr() as *const c_char,
                get: fs_get,
                lookup: fs_lookup,
                child: fs_child,
                read: fs_read,
                volume: fs_volume,
                create: Some(fs_create),
                remove: Some(fs_remove),
                move_to: Some(fs_move),
                set_info: Some(fs_set_info),
                write: Some(fs_write),
                set_length: Some(fs_set_length),
            });
            (fs, ops, cname)
        });
        let mut card_vram = card.map(|_| vec![0u8; CARD_VRAM_SIZE].into_boxed_slice());
        let ops = HostOps {
            card_vram: card_vram
                .as_mut()
                .map_or(core::ptr::null_mut(), |v| v.as_mut_ptr()),
            card_res: card.map_or(0, |c| c.resolution),
            card_depth: card.map_or(0, |c| c.depth),
            fs: fs
                .as_ref()
                .map_or(core::ptr::null(), |(_, ops, _)| &**ops as *const FsOps),
            audio: audio_trampoline,
            audio_ctx: &mut *audio as *mut AudioSink as *mut c_void,
            disk_read: disk_read_trampoline,
            disk_write: disk_write_trampoline,
            disk_blocks: disk.blocks(),
            disk_read_only: disk.read_only(),
            ctx: &mut **disk as *mut Box<dyn MacDisk> as *mut c_void,
            more_disks,
        };
        let Ok(rom_size) = u32::try_from(rom.len()) else {
            VM_IN_USE.store(false, Ordering::Release);
            return Err(StartError::BadArguments);
        };
        // SAFETY: the ROM, RAM and video RAM buffers have the sizes
        // passed (the video RAM CLASSIC_VRAM_SIZE) and are kept by the Mac
        // until it is dropped; `ops` is copied by the callee and its `ctx`s
        // point at the boxed disks the Mac also keeps; `xpram` is 256
        // bytes; the file system's ops (kept, with their ctx and name, by
        // the Mac) stay valid while it lives, and so does the card's video
        // RAM (CLASSIC_CARD_VRAM_SIZE bytes); no other machine uses the C
        // globals (VM_IN_USE).
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
                _disks: disks,
                audio,
                _fs: fs,
                _card_vram: card_vram,
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

    /// The sound the Mac made since the last call: unsigned 8-bit mono
    /// samples and their rate (0 if none yet).
    pub fn take_audio(&mut self) -> (alloc::vec::Vec<u8>, u32) {
        (core::mem::take(&mut self.audio.samples), self.audio.rate)
    }

    /// Makes the system beep (SysBeep), as an application would; `false`
    /// if a call is under way. For tests, on an idle desktop.
    pub fn sysbeep(&mut self) -> bool {
        // SAFETY: plain call; the model queues the call for its next run.
        unsafe { classic_sysbeep() == 0 }
    }

    /// The external file system's state.
    pub fn fs_status(&self) -> FsStatus {
        let (mut install, mut mounted, mut requests) = (0i32, false, 0u32);
        // SAFETY: three valid pointers to locals.
        unsafe { classic_fs_status(&mut install, &mut mounted, &mut requests) };
        FsStatus {
            install: (install != 1).then_some(install),
            mounted,
            requests,
        }
    }

    /// The mounted volumes: (drive number, name), in the File Manager's
    /// order (the startup volume first).
    pub fn volumes(&self) -> alloc::vec::Vec<(u16, alloc::string::String)> {
        const MAX: usize = 8;
        let mut drives = [0u16; MAX];
        let mut names = [[0u8; 28]; MAX];
        // SAFETY: both arrays hold MAX entries; reads guest RAM through the
        // model's bus.
        let n = unsafe { classic_volumes(drives.as_mut_ptr(), names.as_mut_ptr(), MAX as c_int) };
        (0..(n.max(0) as usize).min(MAX))
            .map(|i| {
                let name = names[i]
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| mac_roman_char(b))
                    .collect();
                (drives[i], name)
            })
            .collect()
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
