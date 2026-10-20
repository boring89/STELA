// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA system-call ABI, shared by the kernel and user programs.
//!
//! A program enters the kernel with the `syscall` instruction:
//!
//! ```text
//!  in:   RAX = number (Syscall)   RDI RSI RDX R10 R8 R9 = arguments 1-6
//!  out:  RAX = status (0 = OK, otherwise an Error value)
//!        RDX = the result value (meaningful when the status is OK)
//!  lost: RCX, R11 (the processor uses them for the return address and
//!        RFLAGS); every other register is preserved
//! ```
//!
//! R10 replaces RCX as the fourth argument because `syscall` overwrites
//! RCX (Intel SDM Vol. 2B "SYSCALL").

#![no_std]
#![forbid(unsafe_code)]

pub mod key;

/// System-call numbers (RAX).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Syscall {
    /// `ProcessExit(code)`: ends every thread of the calling process. Does
    /// not return.
    ProcessExit = 0,
    /// `ThreadExit()`: ends the calling thread; the process ends with its
    /// last thread (exit code 0). Does not return.
    ThreadExit = 1,
    /// `ThreadCreate(entry, stack_top, arg) -> thread id`: a new thread of
    /// the calling process starts at `entry` with RDI = `arg` and RSP =
    /// `stack_top` rounded down to 16, minus 8 (as after a `call`).
    ThreadCreate = 2,
    /// `Yield()`: lets other threads run.
    Yield = 3,
    /// `SleepNs(ns)`: sleeps at least `ns` nanoseconds.
    SleepNs = 4,
    /// `ClockNowNs() -> ns`: nanoseconds since boot, monotonic.
    ClockNowNs = 5,
    /// `DebugWrite(ptr, len) -> len`: writes bytes (UTF-8, invalid
    /// sequences replaced) to the kernel console. Temporary, until there
    /// are handles and a console service.
    DebugWrite = 6,
    /// `HandleClose(handle)`: removes the handle from the caller's table.
    HandleClose = 7,
    /// `HandleDuplicate(handle, rights) -> handle`: a new handle to the
    /// same object with `rights`, which must be a subset of the original's
    /// (and the original needs `DUPLICATE`).
    HandleDuplicate = 8,
    /// `PortCreate() -> handle`: a new port; the handle has `SEND |
    /// RECEIVE | DUPLICATE | TRANSFER`.
    PortCreate = 9,
    /// `PortSend(port, &Message)`: queues a copy of the message (and moves
    /// the handles it carries out of the caller's table). Never blocks:
    /// `Full` when 64 messages are waiting.
    PortSend = 10,
    /// `PortReceive(port, &mut Message, timeout_ns)`: takes the oldest
    /// message, waiting up to `timeout_ns` (0: do not wait; `u64::MAX`:
    /// forever).
    PortReceive = 11,
    /// `MemoryCreate(size) -> handle`: zeroed shared memory, `size` a
    /// multiple of 4096 up to 64 MiB.
    MemoryCreate = 12,
    /// `MemoryMap(memory, flags) -> address`: maps the whole object into
    /// the caller's map area (`MAP_WRITE` needs `WRITE`).
    MemoryMap = 13,
    /// `MemoryUnmap(address)`: removes a mapping made by `MemoryMap`.
    MemoryUnmap = 14,
    /// `InputAttach(port)`: keyboard events go to `port` (a handle with
    /// `SEND`) as `INPUT_KEY` messages, replacing the previous receiver.
    InputAttach = 15,
}

impl Syscall {
    /// Every system call, in number order.
    pub const ALL: [Syscall; 16] = [
        Syscall::ProcessExit,
        Syscall::ThreadExit,
        Syscall::ThreadCreate,
        Syscall::Yield,
        Syscall::SleepNs,
        Syscall::ClockNowNs,
        Syscall::DebugWrite,
        Syscall::HandleClose,
        Syscall::HandleDuplicate,
        Syscall::PortCreate,
        Syscall::PortSend,
        Syscall::PortReceive,
        Syscall::MemoryCreate,
        Syscall::MemoryMap,
        Syscall::MemoryUnmap,
        Syscall::InputAttach,
    ];

    /// The call with number `raw`, if any.
    pub const fn from_raw(raw: u64) -> Option<Syscall> {
        if raw < Syscall::ALL.len() as u64 {
            Some(Syscall::ALL[raw as usize])
        } else {
            None
        }
    }

    /// The number.
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

/// Error statuses (RAX after a failed call). 0 means success and is not
/// an `Error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Error {
    /// The number in RAX is not a system call.
    InvalidSyscall = 1,
    /// An argument is out of range.
    InvalidArgument = 2,
    /// A pointer argument is not mapped, readable (or writable) user
    /// memory.
    BadAddress = 3,
    /// The kernel ran out of memory.
    OutOfMemory = 4,
    /// Valid, but not available here.
    NotSupported = 5,
    /// The handle is not in the caller's table (never was, closed, or
    /// moved away).
    BadHandle = 6,
    /// The handle lacks a right the call needs, or more rights were asked
    /// for than it has.
    AccessDenied = 7,
    /// The handle refers to the wrong kind of object.
    WrongType = 8,
    /// The port's queue is full.
    Full = 9,
    /// Nothing arrived within the timeout.
    TimedOut = 10,
    /// The other side is gone: the port was closed.
    PeerClosed = 11,
    /// The handle table is full.
    OutOfHandles = 12,
    /// Something is already mapped there, or the map area is full.
    AlreadyMapped = 13,
}

impl Error {
    /// Every error, in value order.
    pub const ALL: [Error; 13] = [
        Error::InvalidSyscall,
        Error::InvalidArgument,
        Error::BadAddress,
        Error::OutOfMemory,
        Error::NotSupported,
        Error::BadHandle,
        Error::AccessDenied,
        Error::WrongType,
        Error::Full,
        Error::TimedOut,
        Error::PeerClosed,
        Error::OutOfHandles,
        Error::AlreadyMapped,
    ];

    /// The error with status `raw` (`None` for 0 and unknown values).
    pub const fn from_raw(raw: u64) -> Option<Error> {
        if raw >= 1 && raw <= Error::ALL.len() as u64 {
            Some(Error::ALL[raw as usize - 1])
        } else {
            None
        }
    }

    /// The status value.
    pub const fn raw(self) -> u64 {
        self as u64
    }

    /// A short name for messages.
    pub const fn name(self) -> &'static str {
        match self {
            Error::InvalidSyscall => "invalid system call",
            Error::InvalidArgument => "invalid argument",
            Error::BadAddress => "bad address",
            Error::OutOfMemory => "out of memory",
            Error::NotSupported => "not supported",
            Error::BadHandle => "bad handle",
            Error::AccessDenied => "access denied",
            Error::WrongType => "wrong object type",
            Error::Full => "queue full",
            Error::TimedOut => "timed out",
            Error::PeerClosed => "peer closed",
            Error::OutOfHandles => "out of handles",
            Error::AlreadyMapped => "already mapped",
        }
    }
}

/// A reference to a kernel object in a process's handle table: the low 24
/// bits are the slot index, the high 8 bits the slot's generation, which
/// changes every time the slot is freed so that an old handle value stops
/// working (`BadHandle`). Generations start at 1, so 0 is never a valid
/// handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Handle(pub u32);

impl Handle {
    /// Never a valid handle.
    pub const NONE: Handle = Handle(0);
    /// Bits of the slot index.
    pub const INDEX_BITS: u32 = 24;

    /// The handle for slot `index` in generation `generation`.
    pub const fn new(index: u32, generation: u8) -> Handle {
        Handle((generation as u32) << Handle::INDEX_BITS | (index & 0x00FF_FFFF))
    }

    /// The slot index.
    pub const fn index(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }

    /// The slot generation.
    pub const fn generation(self) -> u8 {
        (self.0 >> Handle::INDEX_BITS) as u8
    }

    /// The raw value (a system-call argument).
    pub const fn raw(self) -> u64 {
        self.0 as u64
    }

    /// The handle with raw value `raw` (`NONE` if it does not fit 32 bits).
    pub const fn from_raw(raw: u64) -> Handle {
        if raw > u32::MAX as u64 {
            Handle::NONE
        } else {
            Handle(raw as u32)
        }
    }
}

/// What a handle allows, as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Rights(pub u32);

impl Rights {
    /// No rights.
    pub const NONE: Rights = Rights(0);
    /// `HandleDuplicate` is allowed.
    pub const DUPLICATE: Rights = Rights(1 << 0);
    /// The handle may be sent in a message.
    pub const TRANSFER: Rights = Rights(1 << 1);
    /// `PortSend` (and `InputAttach`).
    pub const SEND: Rights = Rights(1 << 2);
    /// `PortReceive`.
    pub const RECEIVE: Rights = Rights(1 << 3);
    /// Read the object (map memory readable).
    pub const READ: Rights = Rights(1 << 4);
    /// Write the object (map memory writable).
    pub const WRITE: Rights = Rights(1 << 5);
    /// `MemoryMap`.
    pub const MAP: Rights = Rights(1 << 6);
    /// Wait for the object.
    pub const WAIT: Rights = Rights(1 << 7);
    /// Every right defined so far.
    pub const ALL: Rights = Rights(0xFF);

    /// Whether every right in `other` is in `self`.
    pub const fn contains(self, other: Rights) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets together.
    pub const fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }

    /// `self` without the rights in `other`.
    pub const fn without(self, other: Rights) -> Rights {
        Rights(self.0 & !other.0)
    }

    /// The raw bits (a system-call argument).
    pub const fn raw(self) -> u64 {
        self.0 as u64
    }

    /// The rights with bits `raw`, if they are all defined.
    pub const fn from_raw(raw: u64) -> Option<Rights> {
        if raw & !(Rights::ALL.0 as u64) != 0 {
            None
        } else {
            Some(Rights(raw as u32))
        }
    }
}

impl core::ops::BitOr for Rights {
    type Output = Rights;
    fn bitor(self, other: Rights) -> Rights {
        self.union(other)
    }
}

/// Most data bytes in a [`Message`].
pub const MESSAGE_DATA: usize = 112;
/// Most handles in a [`Message`].
pub const MESSAGE_HANDLES: usize = 4;
/// Messages a port holds before `PortSend` returns `Full`.
pub const PORT_QUEUE_DEPTH: usize = 64;

/// A port message: a tag, up to 112 bytes of data and up to 4 handles.
///
/// ```text
///  offset  0  tag            u32   what the message means (abi::tag)
///          4  len            u32   valid bytes in data (<= 112)
///          8  handles        u32   valid entries in handle_slots (<= 4)
///         12  reserved       u32   0
///         16  data           [u8; 112]
///        128  handle_slots   [Handle; 4]
///        144  (size)
/// ```
///
/// Sending moves the handles in `handle_slots[..handles]` (each needs
/// `TRANSFER`) out of the sender's table; receiving puts them in the
/// receiver's table and writes the new values back into the slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Message {
    /// What the message means.
    pub tag: u32,
    /// Valid bytes in `data`.
    pub len: u32,
    /// Valid entries in `handle_slots`.
    pub handles: u32,
    /// Must be 0.
    pub reserved: u32,
    /// The data.
    pub data: [u8; MESSAGE_DATA],
    /// The handles carried.
    pub handle_slots: [Handle; MESSAGE_HANDLES],
}

/// Size of a [`Message`] in bytes.
pub const MESSAGE_SIZE: usize = core::mem::size_of::<Message>();

impl Message {
    /// An empty message with tag 0.
    pub const fn new() -> Message {
        Message {
            tag: 0,
            len: 0,
            handles: 0,
            reserved: 0,
            data: [0; MESSAGE_DATA],
            handle_slots: [Handle::NONE; MESSAGE_HANDLES],
        }
    }

    /// A message with `tag` and as much of `data` as fits (the rest is
    /// cut off).
    pub fn with_data(tag: u32, data: &[u8]) -> Message {
        let mut msg = Message::new();
        msg.tag = tag;
        let len = data.len().min(MESSAGE_DATA);
        msg.data[..len].copy_from_slice(&data[..len]);
        msg.len = len as u32;
        msg
    }

    /// The valid data bytes.
    pub fn payload(&self) -> &[u8] {
        &self.data[..(self.len as usize).min(MESSAGE_DATA)]
    }

    /// The valid handles.
    pub fn carried(&self) -> &[Handle] {
        &self.handle_slots[..(self.handles as usize).min(MESSAGE_HANDLES)]
    }

    /// Adds a handle to carry; `false` when there are already 4.
    pub fn push_handle(&mut self, handle: Handle) -> bool {
        let n = self.handles as usize;
        if n >= MESSAGE_HANDLES {
            return false;
        }
        self.handle_slots[n] = handle;
        self.handles += 1;
        true
    }

    /// The bytes as they are copied to and from user memory.
    pub fn to_bytes(&self) -> [u8; MESSAGE_SIZE] {
        let mut b = [0u8; MESSAGE_SIZE];
        b[0..4].copy_from_slice(&self.tag.to_le_bytes());
        b[4..8].copy_from_slice(&self.len.to_le_bytes());
        b[8..12].copy_from_slice(&self.handles.to_le_bytes());
        b[12..16].copy_from_slice(&self.reserved.to_le_bytes());
        b[16..128].copy_from_slice(&self.data);
        for (i, h) in self.handle_slots.iter().enumerate() {
            b[128 + 4 * i..132 + 4 * i].copy_from_slice(&h.0.to_le_bytes());
        }
        b
    }

    /// The inverse of [`Message::to_bytes`].
    pub fn from_bytes(b: &[u8; MESSAGE_SIZE]) -> Message {
        let word = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        let mut msg = Message::new();
        msg.tag = word(0);
        msg.len = word(4);
        msg.handles = word(8);
        msg.reserved = word(12);
        msg.data.copy_from_slice(&b[16..128]);
        for i in 0..MESSAGE_HANDLES {
            msg.handle_slots[i] = Handle(word(128 + 4 * i));
        }
        msg
    }
}

impl Default for Message {
    fn default() -> Message {
        Message::new()
    }
}

/// Message tags used by the kernel and the system services.
pub mod tag {
    /// To the console service: the data is UTF-8 text to print.
    pub const CONSOLE_WRITE: u32 = 1;
    /// From the kernel to the `InputAttach` port: the data is an encoded
    /// `key::KeyEvent`.
    pub const INPUT_KEY: u32 = 2;
}

/// The handles every process starts with, by slot. A slot that was not
/// given is empty (`BadHandle`).
pub mod bootstrap {
    use super::Handle;

    /// `[0]`: the process's own main port (`RECEIVE | SEND | DUPLICATE |
    /// TRANSFER`).
    pub const MAIN_PORT: Handle = Handle::new(0, 1);
    /// `[1]`: `SEND` to the console service.
    pub const CONSOLE: Handle = Handle::new(1, 1);
    /// `[2]`: a `SEND` handle from whoever started the process.
    pub const BOOTSTRAP: Handle = Handle::new(2, 1);
    /// Number of bootstrap slots.
    pub const COUNT: u32 = 3;
}

/// `MemoryMap` flags.
pub mod map {
    /// Map writable (the handle needs `WRITE`); otherwise read-only.
    pub const WRITE: u64 = 1 << 0;
    /// Every defined flag.
    pub const ALL: u64 = WRITE;
}

/// Largest `MemoryCreate` size.
pub const MEMORY_MAX: u64 = 64 << 20;

/// The status meaning success.
pub const OK: u64 = 0;

/// Turns the (RAX, RDX) pair a call returned into a `Result`. An unknown
/// non-zero status counts as `InvalidSyscall`.
pub const fn decode(status: u64, value: u64) -> Result<u64, Error> {
    if status == OK {
        Ok(value)
    } else {
        match Error::from_raw(status) {
            Some(e) => Err(e),
            None => Err(Error::InvalidSyscall),
        }
    }
}

/// Turns a kernel-side result into the (RAX, RDX) pair.
pub const fn encode(result: Result<u64, Error>) -> (u64, u64) {
    match result {
        Ok(value) => (OK, value),
        Err(e) => (e.raw(), 0),
    }
}

/// The layout of a user address space.
pub mod layout {
    /// End of the user half (exclusive): the lowest non-canonical address.
    pub const USER_END: u64 = 0x0000_8000_0000_0000;
    /// Where programs are linked (`user/user.ld`).
    pub const PROGRAM_BASE: u64 = 0x0000_0000_0040_0000;
    /// Top of the main thread's stack (exclusive).
    pub const STACK_TOP: u64 = 0x0000_7FFF_FFF0_0000;
    /// Size of the main thread's stack, all mapped at start.
    pub const STACK_SIZE: u64 = 1 << 20;
    /// The unmapped page below the stack: running into it page faults.
    pub const STACK_GUARD: u64 = STACK_TOP - STACK_SIZE - 4096;
    /// Start of the area where `MemoryMap` places mappings.
    pub const MAP_BASE: u64 = 0x0000_1000_0000_0000;
    /// End of the map area (exclusive).
    pub const MAP_END: u64 = 0x0000_7000_0000_0000;
}

// The layout's invariants, checked at compile time.
const _: () = {
    use layout::*;
    assert!(PROGRAM_BASE.is_multiple_of(4096));
    assert!(STACK_TOP.is_multiple_of(4096) && STACK_TOP < USER_END);
    assert!(STACK_SIZE.is_multiple_of(4096));
    assert!(PROGRAM_BASE < STACK_GUARD);
    assert!(MAP_BASE.is_multiple_of(4096) && MAP_END.is_multiple_of(4096));
    assert!(PROGRAM_BASE < MAP_BASE && MAP_BASE < MAP_END && MAP_END < STACK_GUARD);
};

// The message layout is ABI too.
const _: () = {
    assert!(MESSAGE_SIZE == 144);
    assert!(core::mem::offset_of!(Message, data) == 16);
    assert!(core::mem::offset_of!(Message, handle_slots) == 128);
};

/// Most bytes one `DebugWrite` takes.
pub const DEBUG_WRITE_MAX: u64 = 4096;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_numbers_are_fixed() {
        // The numbers are the ABI; changing one breaks every program.
        assert_eq!(Syscall::ProcessExit.raw(), 0);
        assert_eq!(Syscall::ThreadExit.raw(), 1);
        assert_eq!(Syscall::ThreadCreate.raw(), 2);
        assert_eq!(Syscall::Yield.raw(), 3);
        assert_eq!(Syscall::SleepNs.raw(), 4);
        assert_eq!(Syscall::ClockNowNs.raw(), 5);
        assert_eq!(Syscall::DebugWrite.raw(), 6);
        assert_eq!(Syscall::HandleClose.raw(), 7);
        assert_eq!(Syscall::HandleDuplicate.raw(), 8);
        assert_eq!(Syscall::PortCreate.raw(), 9);
        assert_eq!(Syscall::PortSend.raw(), 10);
        assert_eq!(Syscall::PortReceive.raw(), 11);
        assert_eq!(Syscall::MemoryCreate.raw(), 12);
        assert_eq!(Syscall::MemoryMap.raw(), 13);
        assert_eq!(Syscall::MemoryUnmap.raw(), 14);
        assert_eq!(Syscall::InputAttach.raw(), 15);
        for (i, call) in Syscall::ALL.iter().enumerate() {
            assert_eq!(call.raw(), i as u64);
            assert_eq!(Syscall::from_raw(i as u64), Some(*call));
        }
        assert_eq!(Syscall::from_raw(16), None);
        assert_eq!(Syscall::from_raw(u64::MAX), None);
    }

    #[test]
    fn error_values_are_fixed() {
        assert_eq!(Error::InvalidSyscall.raw(), 1);
        assert_eq!(Error::InvalidArgument.raw(), 2);
        assert_eq!(Error::BadAddress.raw(), 3);
        assert_eq!(Error::OutOfMemory.raw(), 4);
        assert_eq!(Error::NotSupported.raw(), 5);
        assert_eq!(Error::BadHandle.raw(), 6);
        assert_eq!(Error::AccessDenied.raw(), 7);
        assert_eq!(Error::WrongType.raw(), 8);
        assert_eq!(Error::Full.raw(), 9);
        assert_eq!(Error::TimedOut.raw(), 10);
        assert_eq!(Error::PeerClosed.raw(), 11);
        assert_eq!(Error::OutOfHandles.raw(), 12);
        assert_eq!(Error::AlreadyMapped.raw(), 13);
        for e in Error::ALL {
            assert_eq!(Error::from_raw(e.raw()), Some(e));
            assert!(!e.name().is_empty());
        }
        assert_eq!(Error::from_raw(0), None);
        assert_eq!(Error::from_raw(14), None);
    }

    #[test]
    fn encode_decode_round_trip() {
        assert_eq!(encode(Ok(42)), (0, 42));
        assert_eq!(encode(Err(Error::BadAddress)), (3, 0));
        assert_eq!(decode(0, 42), Ok(42));
        assert_eq!(decode(3, 0), Err(Error::BadAddress));
        assert_eq!(decode(999, 0), Err(Error::InvalidSyscall));
        for e in Error::ALL {
            let (status, value) = encode(Err(e));
            assert_eq!(decode(status, value), Err(e));
        }
    }

    #[test]
    fn layout_values() {
        assert_eq!(layout::STACK_GUARD, 0x7FFF_FFDF_F000);
        assert_eq!(layout::STACK_TOP - layout::STACK_SIZE, 0x7FFF_FFE0_0000);
        assert_eq!(layout::MAP_BASE, 0x1000_0000_0000);
    }

    #[test]
    fn handles_and_rights() {
        let h = Handle::new(5, 3);
        assert_eq!(h.0, 0x0300_0005);
        assert_eq!((h.index(), h.generation()), (5, 3));
        assert_eq!(Handle::from_raw(h.raw()), h);
        assert_eq!(Handle::from_raw(1 << 32), Handle::NONE);
        assert_eq!(bootstrap::MAIN_PORT.0, 0x0100_0000);
        assert_eq!(bootstrap::CONSOLE.0, 0x0100_0001);
        assert_eq!(bootstrap::BOOTSTRAP.0, 0x0100_0002);
        assert_eq!(Rights::DUPLICATE.0, 1);
        assert_eq!(Rights::TRANSFER.0, 2);
        assert_eq!(Rights::SEND.0, 4);
        assert_eq!(Rights::RECEIVE.0, 8);
        assert_eq!(Rights::READ.0, 16);
        assert_eq!(Rights::WRITE.0, 32);
        assert_eq!(Rights::MAP.0, 64);
        assert_eq!(Rights::WAIT.0, 128);
        let r = Rights::SEND | Rights::RECEIVE;
        assert!(r.contains(Rights::SEND));
        assert!(!r.contains(Rights::SEND | Rights::MAP));
        assert_eq!(r.without(Rights::SEND), Rights::RECEIVE);
        assert_eq!(Rights::from_raw(0x100), None);
        assert_eq!(Rights::from_raw(0xFF), Some(Rights::ALL));
    }

    #[test]
    fn message_layout() {
        assert_eq!(MESSAGE_SIZE, 144);
        assert_eq!(core::mem::align_of::<Message>(), 4);
        let mut m = Message::with_data(tag::CONSOLE_WRITE, b"hello");
        assert_eq!(m.payload(), b"hello");
        assert!(m.push_handle(Handle::new(1, 1)));
        let b = m.to_bytes();
        assert_eq!(&b[0..4], &1u32.to_le_bytes());
        assert_eq!(&b[4..8], &5u32.to_le_bytes());
        assert_eq!(&b[8..12], &1u32.to_le_bytes());
        assert_eq!(&b[16..21], b"hello");
        assert_eq!(&b[128..132], &0x0100_0001u32.to_le_bytes());
        assert_eq!(Message::from_bytes(&b), m);
        for _ in 0..3 {
            assert!(m.push_handle(Handle::NONE));
        }
        assert!(!m.push_handle(Handle::NONE));
        assert_eq!(m.carried().len(), 4);
        let long = [7u8; 200];
        assert_eq!(Message::with_data(0, &long).len, 112);
        assert_eq!(tag::CONSOLE_WRITE, 1);
        assert_eq!(tag::INPUT_KEY, 2);
    }
}
