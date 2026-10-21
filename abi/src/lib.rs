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
pub mod window;

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
    // 6 was `DebugWrite`, removed in P4 (the console service replaced
    // it); the number stays unused.
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
    /// `InputAttach(port, input)`: keyboard events go to `port` (a handle
    /// with `SEND`) as `INPUT_KEY` messages, replacing the previous
    /// receiver. `input` is a `capability::INPUT` handle (P11).
    InputAttach = 15,
    /// `DirOpen(dir, path_ptr, path_len, flags) -> handle`: opens (or with
    /// `open::CREATE` makes) the file or folder at the relative path
    /// (components separated by `/`; no `..`).
    DirOpen = 16,
    /// `DirRead(dir, cookie, &mut DirEntry) -> cookie`: the entry after
    /// `cookie` (0: the first) and the cookie for the next call (never 0);
    /// 0 past the last (P11; the cookie is opaque, `docs/vfs.md` §4).
    DirRead = 17,
    /// `DirRemove(dir, name_ptr, name_len)`: removes a file or an empty
    /// folder.
    DirRemove = 18,
    /// `DirRename(dir, old_ptr, old_len, new_dir, new_ptr, new_len)`:
    /// moves and/or renames (same volume).
    DirRename = 19,
    /// `FileRead(file, buf, len, offset) -> bytes`.
    FileRead = 20,
    /// `FileWrite(file, buf, len, offset) -> bytes`.
    FileWrite = 21,
    /// `FileStat(file_or_dir, &mut Stat)`.
    FileStat = 22,
    /// `FileSetSize(file, size)`.
    FileSetSize = 23,
    /// `VolumeList(index, &mut VolumeInfo) -> 1 | 0`.
    VolumeList = 24,
    /// `VolumeOpen(name_ptr, name_len) -> handle`: the root folder of the
    /// volume with that name.
    VolumeOpen = 25,
    /// `VolumeOpenById(dir, id) -> handle`: the item with persistent id
    /// `id` on `dir`'s volume.
    VolumeOpenById = 26,
    /// `ProcessSpawn(file, &Message, &[Handle; n], n) -> handle`: starts
    /// the ELF program in `file` (a handle with `READ`) as a new process.
    /// The message (or an empty one) is queued on its main port as its
    /// `BOOTSTRAP_ARGS`; the handles (n ≤ 6) become its `[1]`..`[n]` (moved, each
    /// needs `TRANSFER`; `Handle::NONE` leaves a slot empty). The handle
    /// returned has `WAIT | KILL | DUPLICATE | TRANSFER`. Arguments 5 and
    /// 6 (P11): `&[u32; 2 * n]` pairs (handle, `capability::*` kind) and
    /// `n` (≤ `capability::MAX`), moved into the `CAPABILITIES` message the
    /// kernel queues second (empty when `n` is 0).
    ProcessSpawn = 27,
    /// `ProcessWait(process, timeout_ns) -> exit code`: waits until the
    /// process ended (`TimedOut` otherwise); the code is an `i64`.
    ProcessWait = 28,
    /// `ProcessKill(process)`: ends it with exit code `EXIT_KILLED`.
    ProcessKill = 29,
    /// `ProcessSelf() -> handle`: the caller's own process (`KILL |
    /// DUPLICATE | TRANSFER`).
    ProcessSelf = 30,
    /// `SysInfo(kind, buf, len) -> count`: system information
    /// (`sysinfo::*`).
    SysInfo = 31,
    /// `SystemPower(kind, power)`: flushes and unmounts the volumes, then
    /// turns the machine off (`power::SHUTDOWN`) or restarts it
    /// (`power::REBOOT`). `power` is a `capability::POWER` handle
    /// (`AccessDenied` without). Returns only on failure (`NotSupported`:
    /// no ACPI soft off on this machine).
    SystemPower = 32,
    /// `TtyCreate(flags) -> handles`: a new terminal (`docs/linux.md` §5).
    /// The low 32 bits of the value are the program-side handle (`SEND |
    /// DUPLICATE | TRANSFER`: `PortSend` of `CONSOLE_WRITE` prints,
    /// `INPUT_KEY` types); with `tty::MASTER` the high 32 bits are the
    /// master handle (`READ | WRITE | DUPLICATE | TRANSFER`) and the
    /// output waits for `TtyRead` instead of going to the kernel console.
    TtyCreate = 33,
    /// `TtyRead(master, buf, len, timeout_ns) -> bytes`: the terminal's
    /// output (waits like `PortReceive`; `TimedOut`).
    TtyRead = 34,
    /// `TtyWrite(master, buf, len) -> bytes`: typed input (UTF-8 text,
    /// through the line discipline).
    TtyWrite = 35,
    /// `HandleKind(handle) -> kind`: what the handle refers to
    /// (`object_kind::*`).
    HandleKind = 36,
    /// `FileGetInfo(file or folder, &mut [u8; 32])`: the item's Finder
    /// information (`FINDER_INFO_SIZE` bytes; Mouseion's window and icon
    /// positions). `NotSupported` on volumes that do not keep it.
    FileGetInfo = 37,
    /// `FileSetInfo(file or folder, &[u8; 32])`: stores it (needs
    /// `WRITE` on the handle).
    FileSetInfo = 38,
    /// `SocketCreate(kind, network) -> handle`: a network socket
    /// (`net::UDP`, `net::TCP`, `net::ICMP`; `docs/net.md`). `network` is
    /// a `capability::NETWORK` handle (P11).
    SocketCreate = 39,
    /// `SocketBind(socket, port) -> port`: the local port (0: any free
    /// one).
    SocketBind = 40,
    /// `SocketConnect(socket, addr, timeout)`: TCP connects to `addr`
    /// (`net::pack`), waiting up to `timeout` ns; UDP sets the default
    /// destination.
    SocketConnect = 41,
    /// `SocketListen(socket, backlog)`: TCP waits for connections on its
    /// bound port.
    SocketListen = 42,
    /// `SocketAccept(socket, timeout) -> handle`: the next connection.
    SocketAccept = 43,
    /// `SocketSend(socket, buf, len, addr) -> bytes`: TCP queues data
    /// (waiting for room); UDP sends one datagram to `addr` (0: the
    /// connected peer); ICMP sends an echo request (`addr`'s port is the
    /// sequence number).
    SocketSend = 44,
    /// `SocketRecv(socket, buf, len, timeout, &mut addr) -> bytes`: TCP
    /// reads (0 at the end of the stream); UDP and ICMP take one datagram
    /// and write its source (`net::pack`, 8 bytes) if `addr` is not 0.
    SocketRecv = 45,
    /// `NetInfo(&mut [u8; 48]) -> 0 or 1`: the network card and address
    /// (`net::NetInfo`); 0 when there is no card.
    NetInfo = 46,
    /// `AudioOpen(rate, channels, &mut [u64; 2])`: the sound output, for
    /// one program at a time (`Busy` otherwise); writes the Audio handle
    /// and the handle of its shared ring (`audio::Ring`, `docs/sound.md`).
    AudioOpen = 47,
    /// `AudioClose(audio)`: stops the output and closes the handle.
    AudioClose = 48,
    /// `WaitSetCreate() -> handle`: an empty set of objects to wait on
    /// together (`docs/ipc.md` §13).
    WaitSetCreate = 49,
    /// `WaitSetAdd(set, handle, key, mask)`: adds (or updates) the object
    /// `handle` refers to, reported with `key` for the `wait::` events in
    /// `mask` (0: all).
    WaitSetAdd = 50,
    /// `WaitSetRemove(set, handle)`: removes the object.
    WaitSetRemove = 51,
    /// `WaitSetWait(set, &mut [wait::Ready], max, timeout_ns) -> count`:
    /// waits until a member is ready and writes up to `max` records.
    WaitSetWait = 52,
    /// `FilePath(item, buf, len) -> length`: the item's path inside its
    /// volume (`/` for the root, `/a/b`; UTF-8, no NUL); the whole length
    /// is returned even when `len` cut it (P11, `docs/vfs.md` §4.4).
    FilePath = 53,
    /// `PipeCreate(&mut [u32; 2])`: a pipe (P11, `docs/ipc.md` §14); writes
    /// the reading end's handle (`READ | WAIT | DUPLICATE | TRANSFER`) and
    /// the writing end's (`WRITE | SEND | WAIT | DUPLICATE | TRANSFER`).
    PipeCreate = 54,
    /// `PipeRead(read_end, buf, len, timeout_ns) -> bytes`: waits until
    /// there are some; 0 at the end (every writing handle gone).
    PipeRead = 55,
    /// `PipeWrite(write_end, buf, len, timeout_ns) -> bytes`: waits for
    /// room; `PeerClosed` once every reading handle is gone.
    PipeWrite = 56,
    /// `TtySetSize(master, columns, rows)`: what the terminal shows, for
    /// Linux's `TIOCGWINSZ` (P11; 80 x 25 until set).
    TtySetSize = 57,
}

impl Syscall {
    /// Every system call, in number order.
    pub const ALL: [Syscall; 57] = [
        Syscall::ProcessExit,
        Syscall::ThreadExit,
        Syscall::ThreadCreate,
        Syscall::Yield,
        Syscall::SleepNs,
        Syscall::ClockNowNs,
        Syscall::HandleClose,
        Syscall::HandleDuplicate,
        Syscall::PortCreate,
        Syscall::PortSend,
        Syscall::PortReceive,
        Syscall::MemoryCreate,
        Syscall::MemoryMap,
        Syscall::MemoryUnmap,
        Syscall::InputAttach,
        Syscall::DirOpen,
        Syscall::DirRead,
        Syscall::DirRemove,
        Syscall::DirRename,
        Syscall::FileRead,
        Syscall::FileWrite,
        Syscall::FileStat,
        Syscall::FileSetSize,
        Syscall::VolumeList,
        Syscall::VolumeOpen,
        Syscall::VolumeOpenById,
        Syscall::ProcessSpawn,
        Syscall::ProcessWait,
        Syscall::ProcessKill,
        Syscall::ProcessSelf,
        Syscall::SysInfo,
        Syscall::SystemPower,
        Syscall::TtyCreate,
        Syscall::TtyRead,
        Syscall::TtyWrite,
        Syscall::HandleKind,
        Syscall::FileGetInfo,
        Syscall::FileSetInfo,
        Syscall::SocketCreate,
        Syscall::SocketBind,
        Syscall::SocketConnect,
        Syscall::SocketListen,
        Syscall::SocketAccept,
        Syscall::SocketSend,
        Syscall::SocketRecv,
        Syscall::NetInfo,
        Syscall::AudioOpen,
        Syscall::AudioClose,
        Syscall::WaitSetCreate,
        Syscall::WaitSetAdd,
        Syscall::WaitSetRemove,
        Syscall::WaitSetWait,
        Syscall::FilePath,
        Syscall::PipeCreate,
        Syscall::PipeRead,
        Syscall::PipeWrite,
        Syscall::TtySetSize,
    ];

    /// The call with number `raw`, if any.
    pub const fn from_raw(raw: u64) -> Option<Syscall> {
        let mut i = 0;
        while i < Syscall::ALL.len() {
            if Syscall::ALL[i] as u64 == raw {
                return Some(Syscall::ALL[i]);
            }
            i += 1;
        }
        None
    }

    /// The number.
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

/// The number `DebugWrite` had (P3); removed, so it is `InvalidSyscall`.
pub const REMOVED_DEBUG_WRITE: u64 = 6;

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
    /// No such file, folder or volume.
    NotFound = 14,
    /// The name is taken.
    Exists = 15,
    /// Not a folder.
    NotDir = 16,
    /// A folder where a file was expected.
    IsDir = 17,
    /// The folder is not empty.
    NotEmpty = 18,
    /// The volume is read-only.
    ReadOnly = 19,
    /// The volume is full.
    NoSpace = 20,
    /// The device failed or the volume is damaged.
    IoError = 21,
    /// Nothing listens at the address (TCP RST to the SYN).
    ConnectionRefused = 22,
    /// The peer reset the connection.
    ConnectionReset = 23,
    /// No network card, or no address yet (DHCP not done).
    NetworkDown = 24,
    /// The port is taken.
    AddressInUse = 25,
    /// The device is in use by another program.
    Busy = 26,
}

impl Error {
    /// Every error, in value order.
    pub const ALL: [Error; 26] = [
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
        Error::NotFound,
        Error::Exists,
        Error::NotDir,
        Error::IsDir,
        Error::NotEmpty,
        Error::ReadOnly,
        Error::NoSpace,
        Error::IoError,
        Error::ConnectionRefused,
        Error::ConnectionReset,
        Error::NetworkDown,
        Error::AddressInUse,
        Error::Busy,
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
            Error::NotFound => "not found",
            Error::Exists => "already exists",
            Error::NotDir => "not a folder",
            Error::IsDir => "is a folder",
            Error::NotEmpty => "folder not empty",
            Error::ReadOnly => "read-only volume",
            Error::NoSpace => "no space left",
            Error::ConnectionRefused => "connection refused",
            Error::ConnectionReset => "connection reset",
            Error::NetworkDown => "network down",
            Error::AddressInUse => "address in use",
            Error::Busy => "device busy",
            Error::IoError => "I/O error",
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
    /// Wait for the object (`ProcessWait`).
    pub const WAIT: Rights = Rights(1 << 7);
    /// End the object (`ProcessKill`).
    pub const KILL: Rights = Rights(1 << 8);
    /// Change a file's or folder's metadata (`FileSetInfo`; P11).
    pub const MANAGE: Rights = Rights(1 << 9);
    /// Every right defined so far.
    pub const ALL: Rights = Rights(0x3FF);

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
    /// The first message on a new process's main port: its arguments,
    /// NUL-separated (the program name first). Up to 112 bytes are in the
    /// data; longer lists are in a `Memory` object carried in slot 0, with
    /// the length (u64, little endian) in the data.
    pub const BOOTSTRAP_ARGS: u32 = 3;
    /// From a shell to its parent (init): handle slot 0 is a `SEND` handle
    /// to the shell's main port, for keyboard input.
    pub const SHELL_READY: u32 = 4;
    /// A mouse event (`key::MouseEvent::encode`, 8 bytes), from the kernel
    /// to the port attached with `InputAttach`.
    pub const INPUT_MOUSE: u32 = 5;
    /// From a terminal to its shell: handle slot 0 is a `SEND` handle to
    /// the window server, for the programs the shell starts (their `[2]`).
    pub const WINDOW_SERVER: u32 = 6;
    /// The second message on a new STELA process's main port (P11): the
    /// capability handles it was given, their `capability::*` kinds as u32
    /// LE in the data, one per carried handle (none: empty).
    pub const CAPABILITIES: u32 = 7;
    /// To init (the `capability::INIT` port): turn off or restart
    /// (`power::*`, u64 LE). Slot 0 may carry a `SEND` handle for the
    /// answer, sent back with the same tag and an `Error` value (u64 LE)
    /// when it fails (on success the machine is gone).
    pub const POWER_REQUEST: u32 = 8;
}

/// Capabilities (P11, `docs/userland.md` §2.3): handles whose possession
/// is the permission. The kernel gives init `POWER`, `INPUT` and
/// `NETWORK`; init passes them on.
pub mod capability {
    /// `SystemPower`.
    pub const POWER: u32 = 1;
    /// `InputAttach` (taking the keyboard and the mouse).
    pub const INPUT: u32 = 2;
    /// `SocketCreate`.
    pub const NETWORK: u32 = 3;
    /// Not a kernel capability: a `SEND` handle to init's main port, for
    /// `tag::POWER_REQUEST`.
    pub const INIT: u32 = 4;
    /// Most capabilities one `ProcessSpawn` passes (and a message
    /// carries).
    pub const MAX: usize = 4;
    /// Whether `kind` is one of the kinds above.
    pub const fn is_valid(kind: u32) -> bool {
        kind >= POWER && kind <= INIT
    }
}

/// The exit code of a process ended by `ProcessKill`.
pub const EXIT_KILLED: i64 = 137;

/// The ELF note that marks a STELA program (in a `PT_NOTE` segment; libstela
/// puts it in every program). Programs without it run with the Linux
/// personality (`docs/linux.md`).
pub mod note {
    /// The note's owner name (without the NUL).
    pub const NAME: &[u8] = b"STELA";
    /// Note type: the STELA ABI; the descriptor is the version (u32 LE).
    pub const ABI: u32 = 1;
    /// The ABI version in the descriptor.
    pub const VERSION: u32 = 1;
    /// The whole note: namesz 6, descsz 4, type 1, "STELA\0" padded to 8,
    /// version 1.
    pub const BYTES: [u8; 24] = [
        6, 0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0, b'S', b'T', b'E', b'L', b'A', 0, 0, 0, 1, 0, 0, 0,
    ];
}

/// `ProcessSpawn` flags, in the high 32 bits of its handle count argument.
pub mod spawn {
    /// Run the program with the Linux personality even if it has the STELA
    /// note.
    pub const FORCE_LINUX: u64 = 1 << 32;
    /// Run the program as a STELA program even without the note.
    pub const FORCE_STELA: u64 = 1 << 33;
    /// Every defined flag.
    pub const ALL: u64 = FORCE_LINUX | FORCE_STELA;
}

/// Bytes of an item's Finder information (`FileGetInfo`, `FileSetInfo`):
/// opaque to the kernel; eHFS keeps it in the catalog record. Mouseion's
/// layout is in `docs/mouseion.md`.
pub const FINDER_INFO_SIZE: usize = 32;

/// Bytes a pipe holds (`PipeCreate`).
pub const PIPE_BUFFER: usize = 64 * 1024;

/// `TtyCreate` flags.
pub mod tty {
    /// Make a master end too (the terminal application's side).
    pub const MASTER: u64 = 1;
}

/// What a handle refers to (`HandleKind`).
pub mod object_kind {
    /// A port.
    pub const PORT: u64 = 1;
    /// Shared memory.
    pub const MEMORY: u64 = 2;
    /// A folder.
    pub const DIRECTORY: u64 = 3;
    /// A file.
    pub const FILE: u64 = 4;
    /// A process.
    pub const PROCESS: u64 = 5;
    /// A terminal's program side.
    pub const TTY: u64 = 6;
    /// A terminal's master side.
    pub const TTY_MASTER: u64 = 7;
    /// The framebuffer (`docs/display.md`).
    pub const FRAMEBUFFER: u64 = 8;
    /// A network socket (`docs/net.md`).
    pub const SOCKET: u64 = 9;
    /// The sound output (`docs/sound.md`).
    pub const AUDIO: u64 = 10;
    /// A set of objects to wait on (`wait`).
    pub const WAIT_SET: u64 = 11;
    /// A capability (`capability`).
    pub const CAPABILITY: u64 = 12;
    /// A pipe's reading end.
    pub const PIPE_READ: u64 = 13;
    /// A pipe's writing end.
    pub const PIPE_WRITE: u64 = 14;
}

/// Waiting on several objects (`WaitSetAdd`, `WaitSetWait`;
/// `docs/ipc.md` §13): the events and the record `WaitSetWait` writes.
pub mod wait {
    /// A port has a message; a socket or terminal has data (or the end of
    /// the stream); a listener has a connection.
    pub const READABLE: u32 = 1;
    /// A socket can take data; the sound ring is at least half empty.
    pub const WRITABLE: u32 = 2;
    /// A TCP connection is established.
    pub const CONNECTED: u32 = 4;
    /// The other side is gone: a port closed and empty, a connection
    /// reset or ended.
    pub const CLOSED: u32 = 8;
    /// A process has ended.
    pub const EXITED: u32 = 16;
    /// Every defined event.
    pub const ALL: u32 = READABLE | WRITABLE | CONNECTED | CLOSED | EXITED;
    /// Most members a set holds.
    pub const MAX_MEMBERS: usize = 256;
    /// Most records one `WaitSetWait` writes.
    pub const MAX_READY: usize = 64;
    /// Bytes of a [`Ready`] record.
    pub const READY_SIZE: usize = 16;

    /// One ready member: its key and the events (in its mask) that hold.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Ready {
        /// The key given to `WaitSetAdd`.
        pub key: u64,
        /// The `wait::` events.
        pub events: u32,
    }

    impl Ready {
        /// The 16-byte record: key (u64 LE), events (u32 LE), 0 (u32).
        pub fn to_bytes(&self) -> [u8; READY_SIZE] {
            let mut b = [0u8; READY_SIZE];
            b[..8].copy_from_slice(&self.key.to_le_bytes());
            b[8..12].copy_from_slice(&self.events.to_le_bytes());
            b
        }

        /// The record from its bytes.
        pub fn from_bytes(b: &[u8; READY_SIZE]) -> Ready {
            let mut key = [0u8; 8];
            key.copy_from_slice(&b[..8]);
            let mut events = [0u8; 4];
            events.copy_from_slice(&b[8..12]);
            Ready {
                key: u64::from_le_bytes(key),
                events: u32::from_le_bytes(events),
            }
        }
    }
}

/// `SystemPower` kinds.
pub mod power {
    /// Turn the machine off.
    pub const SHUTDOWN: u64 = 0;
    /// Restart the machine.
    pub const REBOOT: u64 = 1;
}

/// Networking (`docs/net.md`): socket kinds, addresses packed in a
/// `u64`, the `NetInfo` record.
pub mod net {
    /// Datagrams.
    pub const UDP: u64 = 1;
    /// A byte stream (or a listener).
    pub const TCP: u64 = 2;
    /// ICMP echo (ping).
    pub const ICMP: u64 = 3;

    /// An IPv4 address and port in one `u64`: the address's four bytes
    /// (most significant first) in bits 47..16, the port in bits 15..0.
    pub const fn pack(ip: [u8; 4], port: u16) -> u64 {
        (u32::from_be_bytes(ip) as u64) << 16 | port as u64
    }

    /// The address and port of [`pack`].
    pub const fn unpack(v: u64) -> ([u8; 4], u16) {
        (((v >> 16) as u32).to_be_bytes(), v as u16)
    }

    /// Size of the `NetInfo` record.
    pub const NET_INFO_SIZE: usize = 48;

    /// The network card and the interface's address.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct NetInfo {
        /// The driver's name ("e1000", "e1000e", "virtio-net"), NUL padded.
        pub driver: [u8; 16],
        /// The card's MAC address.
        pub mac: [u8; 6],
        /// The link is up.
        pub link: bool,
        /// The interface has an address.
        pub configured: bool,
        /// Our address.
        pub ip: [u8; 4],
        /// Subnet mask.
        pub mask: [u8; 4],
        /// Default gateway.
        pub gateway: [u8; 4],
        /// DNS server.
        pub dns: [u8; 4],
    }

    impl NetInfo {
        /// The 48-byte record.
        pub fn to_bytes(&self) -> [u8; NET_INFO_SIZE] {
            let mut b = [0u8; NET_INFO_SIZE];
            b[0..16].copy_from_slice(&self.driver);
            b[16..22].copy_from_slice(&self.mac);
            b[22] = self.link as u8;
            b[23] = self.configured as u8;
            b[24..28].copy_from_slice(&self.ip);
            b[28..32].copy_from_slice(&self.mask);
            b[32..36].copy_from_slice(&self.gateway);
            b[36..40].copy_from_slice(&self.dns);
            b
        }

        /// Decodes the record.
        pub fn from_bytes(b: &[u8]) -> Option<NetInfo> {
            if b.len() < NET_INFO_SIZE {
                return None;
            }
            let ip = |at: usize| [b[at], b[at + 1], b[at + 2], b[at + 3]];
            let mut driver = [0u8; 16];
            driver.copy_from_slice(&b[0..16]);
            let mut mac = [0u8; 6];
            mac.copy_from_slice(&b[16..22]);
            Some(NetInfo {
                driver,
                mac,
                link: b[22] != 0,
                configured: b[23] != 0,
                ip: ip(24),
                mask: ip(28),
                gateway: ip(32),
                dns: ip(36),
            })
        }

        /// The driver's name.
        pub fn driver_name(&self) -> &str {
            let end = self.driver.iter().position(|&c| c == 0).unwrap_or(16);
            core::str::from_utf8(&self.driver[..end]).unwrap_or("?")
        }
    }
}

/// Sound (`docs/sound.md`): the shared ring between a program and the
/// kernel's audio thread.
///
/// ```text
///  offset 0     write position (u32): bytes the program has written, ever
///  offset 4     read position (u32): bytes the kernel has taken, ever
///  offset 8     ring size in bytes (u32)          offset 12  rate (u32)
///  offset 16    channels (u32)                    offset 20  underruns (u32):
///                                                 times the ring ran dry mid-piece
///  offset 4096  the samples: 16-bit signed little-endian, interleaved
/// ```
///
/// Positions wrap at 2^32; the bytes in the ring are `write - read`, the
/// room is `size - (write - read)`; byte `p` lives at `4096 + p % size`.
pub mod audio {
    /// The only rate.
    pub const RATE: u32 = 48_000;
    /// The only channel count.
    pub const CHANNELS: u32 = 2;
    /// Bytes per frame (two 16-bit samples).
    pub const FRAME_BYTES: u32 = 4;
    /// Offset of the write position.
    pub const WRITE_POS: usize = 0;
    /// Offset of the read position.
    pub const READ_POS: usize = 4;
    /// Offset of the ring size.
    pub const SIZE: usize = 8;
    /// Offset of the rate.
    pub const RATE_AT: usize = 12;
    /// Offset of the channel count.
    pub const CHANNELS_AT: usize = 16;
    /// Offset of the underrun counter.
    pub const UNDERRUNS: usize = 20;
    /// Offset of the samples.
    pub const DATA: usize = 4096;
    /// Bytes of samples.
    pub const RING_BYTES: u32 = 64 * 1024;
    /// Size of the shared memory.
    pub const MEMORY_SIZE: u64 = DATA as u64 + RING_BYTES as u64;
}

/// `SysInfo` kinds and records.
pub mod sysinfo {
    /// Running and ended processes: an array of `ProcessInfo` (48 bytes
    /// each); returns how many there are (it may be more than fit).
    pub const PROCESSES: u64 = 1;
    /// Memory: one `MemInfo` (32 bytes); returns 1.
    pub const MEMORY: u64 = 2;
    /// Time: `[uptime ns, timer ticks]` (16 bytes); returns 1.
    pub const UPTIME: u64 = 3;
    /// The display: one `FramebufferInfo` (24 bytes); returns 1, or 0
    /// when there is no framebuffer.
    pub const FRAMEBUFFER: u64 = 4;

    /// The boot options (`\\STELA\\boot.cfg`, `docs/p520.md`) as
    /// space-separated `key` or `key=value` words; returns their length
    /// in bytes (at most `CMDLINE_SIZE`).
    pub const CMDLINE: u64 = 5;

    /// The wall clock: nanoseconds since 1970-01-01 00:00 UTC (u64, 8
    /// bytes; the RTC read at boot plus the time since, `docs/time.md`
    /// §5.5); returns 1.
    pub const TIME: u64 = 6;

    /// The processors: one u64 per online processor (logical number
    /// order), the timer ticks it has handled; returns how many there
    /// are (`docs/smp.md`).
    pub const CPUS: u64 = 7;

    /// The running processes' processors: pairs of u64 (process id,
    /// processor number); returns how many pairs there are.
    pub const PROCESS_CPUS: u64 = 8;

    /// Most bytes `SysInfo(CMDLINE)` returns.
    pub const CMDLINE_SIZE: usize = 256;

    /// Looks up an option in a command line made by `bootinfo::parse_config`:
    /// `Some("")` for a bare `key`, `Some(value)` for `key=value`, `None` when
    /// the key is absent. Keys compare ASCII case-insensitively; the last
    /// occurrence wins.
    pub fn cmdline_option<'a>(cmdline: &'a [u8], key: &str) -> Option<&'a [u8]> {
        cmdline.split(|&b| b == b' ').rev().find_map(|word| {
            let (k, v) = match word.iter().position(|&b| b == b'=') {
                Some(i) => (&word[..i], &word[i + 1..]),
                None => (word, &word[word.len()..]),
            };
            k.eq_ignore_ascii_case(key.as_bytes()).then_some(v)
        })
    }

    /// Whether a command line has the option `key` (bare or with a value
    /// other than `off`, `0` or `no`).
    pub fn cmdline_flag(cmdline: &[u8], key: &str) -> bool {
        match cmdline_option(cmdline, key) {
            Some(v) => {
                !(v.eq_ignore_ascii_case(b"off") || v == b"0" || v.eq_ignore_ascii_case(b"no"))
            }
            None => false,
        }
    }

    /// Size of the framebuffer record.
    pub const FRAMEBUFFER_INFO_SIZE: usize = 24;

    /// `FramebufferInfo::format`: byte 0 of a pixel is red.
    pub const FORMAT_RGBX: u32 = 1;
    /// `FramebufferInfo::format`: byte 0 of a pixel is blue.
    pub const FORMAT_BGRX: u32 = 2;

    /// The display (`docs/display.md`).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct FramebufferInfo {
        /// Visible width in pixels.
        pub width: u32,
        /// Visible height in pixels.
        pub height: u32,
        /// Bytes from one row to the next.
        pub pitch: u32,
        /// `FORMAT_RGBX` or `FORMAT_BGRX` (4 bytes per pixel).
        pub format: u32,
        /// Bytes of the framebuffer (what `MemoryMap` maps).
        pub size: u32,
        /// UI scale: 1 (2 is reserved for HiDPI; not used yet).
        pub scale: u32,
    }

    impl FramebufferInfo {
        /// The record as copied to user memory.
        pub fn to_bytes(&self) -> [u8; FRAMEBUFFER_INFO_SIZE] {
            let mut b = [0u8; FRAMEBUFFER_INFO_SIZE];
            for (i, v) in [
                self.width,
                self.height,
                self.pitch,
                self.format,
                self.size,
                self.scale,
            ]
            .iter()
            .enumerate()
            {
                b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            b
        }

        /// The inverse of [`FramebufferInfo::to_bytes`].
        pub fn from_bytes(b: &[u8]) -> Option<FramebufferInfo> {
            let b = b.get(..FRAMEBUFFER_INFO_SIZE)?;
            let w = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            Some(FramebufferInfo {
                width: w(0),
                height: w(4),
                pitch: w(8),
                format: w(12),
                size: w(16),
                scale: w(20),
            })
        }
    }

    /// Size of a process record.
    pub const PROCESS_INFO_SIZE: usize = 48;
    /// Size of the memory record.
    pub const MEM_INFO_SIZE: usize = 32;

    /// One process.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ProcessInfo {
        /// Process id.
        pub pid: u64,
        /// Live threads (0 once ended).
        pub threads: u32,
        /// 1 running, 2 ended.
        pub state: u32,
        /// The exit code once ended.
        pub exit_code: i64,
        /// The name (UTF-8, NUL padded).
        pub name: [u8; 24],
    }

    impl ProcessInfo {
        /// The record as copied to user memory.
        pub fn to_bytes(&self) -> [u8; PROCESS_INFO_SIZE] {
            let mut b = [0u8; PROCESS_INFO_SIZE];
            b[0..8].copy_from_slice(&self.pid.to_le_bytes());
            b[8..12].copy_from_slice(&self.threads.to_le_bytes());
            b[12..16].copy_from_slice(&self.state.to_le_bytes());
            b[16..24].copy_from_slice(&self.exit_code.to_le_bytes());
            b[24..48].copy_from_slice(&self.name);
            b
        }

        /// The inverse of [`ProcessInfo::to_bytes`].
        pub fn from_bytes(b: &[u8]) -> Option<ProcessInfo> {
            let b = b.get(..PROCESS_INFO_SIZE)?;
            let mut name = [0u8; 24];
            name.copy_from_slice(&b[24..48]);
            Some(ProcessInfo {
                pid: u64::from_le_bytes(b[0..8].try_into().ok()?),
                threads: u32::from_le_bytes(b[8..12].try_into().ok()?),
                state: u32::from_le_bytes(b[12..16].try_into().ok()?),
                exit_code: i64::from_le_bytes(b[16..24].try_into().ok()?),
                name,
            })
        }

        /// The name.
        pub fn name(&self) -> &str {
            let n = self.name.iter().position(|&c| c == 0).unwrap_or(24);
            core::str::from_utf8(&self.name[..n]).unwrap_or("?")
        }
    }

    /// Memory.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct MemInfo {
        /// Physical frames (4 KiB) the allocator manages.
        pub total_frames: u64,
        /// Free frames.
        pub free_frames: u64,
        /// Kernel heap size in bytes.
        pub heap_size: u64,
        /// Kernel heap bytes in use.
        pub heap_used: u64,
    }

    impl MemInfo {
        /// The record as copied to user memory.
        pub fn to_bytes(&self) -> [u8; MEM_INFO_SIZE] {
            let mut b = [0u8; MEM_INFO_SIZE];
            b[0..8].copy_from_slice(&self.total_frames.to_le_bytes());
            b[8..16].copy_from_slice(&self.free_frames.to_le_bytes());
            b[16..24].copy_from_slice(&self.heap_size.to_le_bytes());
            b[24..32].copy_from_slice(&self.heap_used.to_le_bytes());
            b
        }

        /// The inverse of [`MemInfo::to_bytes`].
        pub fn from_bytes(b: &[u8]) -> Option<MemInfo> {
            let w = |i: usize| -> Option<u64> {
                Some(u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?))
            };
            Some(MemInfo {
                total_frames: w(0)?,
                free_frames: w(8)?,
                heap_size: w(16)?,
                heap_used: w(24)?,
            })
        }
    }
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
    /// `[3]`: the working directory (the root of `Data` when the kernel
    /// starts a program and there is one).
    pub const CWD: Handle = Handle::new(3, 1);
    /// `[4]` (P11): the folder the program was started from (a Linux
    /// program sees that volume too).
    pub const PROGRAM: Handle = Handle::new(4, 1);
    /// `[5]` (P11): standard input, when given (a pipe or a tty's
    /// reading side).
    pub const STDIN: Handle = Handle::new(5, 1);
    /// `[6]`: reserved.
    pub const RESERVED: Handle = Handle::new(6, 1);
    /// Number of bootstrap slots.
    pub const COUNT: u32 = 7;
    /// How many of them `ProcessSpawn` fills (`[1]`..`[6]`).
    pub const GIVEN: usize = 6;
}

/// `DirOpen` flags.
pub mod open {
    /// The handle may read (files: `FileRead`; folders: list, open inside).
    pub const READ: u64 = 1 << 0;
    /// The handle may write (files: `FileWrite`, `FileSetSize`; folders:
    /// create, remove, rename inside).
    pub const WRITE: u64 = 1 << 1;
    /// Make the item if it does not exist (the folder needs `WRITE`).
    pub const CREATE: u64 = 1 << 2;
    /// Files: cut to length 0 (needs `WRITE`).
    pub const TRUNCATE: u64 = 1 << 3;
    /// The item is (or is made as) a folder.
    pub const DIRECTORY: u64 = 1 << 4;
    /// With `CREATE`: fail with `Exists` if it is there already.
    pub const EXCLUSIVE: u64 = 1 << 5;
    /// Every defined flag.
    pub const ALL: u64 = (1 << 6) - 1;
}

/// What a `DirEntry` or `Stat` describes.
pub mod kind {
    /// A file.
    pub const FILE: u32 = 1;
    /// A folder.
    pub const DIRECTORY: u32 = 2;
}

/// Longest name in a `DirEntry` (bytes of UTF-8).
pub const NAME_MAX: usize = 255;

/// One folder entry (`DirRead`), 288 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct DirEntry {
    /// Persistent id (eHFS: the CNID).
    pub id: u64,
    /// Size in bytes (files).
    pub size: u64,
    /// Last change, ns since 1970 (0 if unknown).
    pub mtime: u64,
    /// `kind::FILE` or `kind::DIRECTORY`.
    pub kind: u32,
    /// Valid bytes of `name`.
    pub name_len: u32,
    /// The name (UTF-8).
    pub name: [u8; NAME_MAX + 1],
}

impl DirEntry {
    /// An empty entry.
    pub const fn new() -> DirEntry {
        DirEntry {
            id: 0,
            size: 0,
            mtime: 0,
            kind: 0,
            name_len: 0,
            name: [0; NAME_MAX + 1],
        }
    }

    /// The name, if it is UTF-8.
    pub fn name(&self) -> &str {
        let n = (self.name_len as usize).min(NAME_MAX);
        core::str::from_utf8(&self.name[..n]).unwrap_or("?")
    }

    /// The bytes copied to user memory.
    pub fn to_bytes(&self) -> [u8; DIR_ENTRY_SIZE] {
        let mut b = [0u8; DIR_ENTRY_SIZE];
        b[0..8].copy_from_slice(&self.id.to_le_bytes());
        b[8..16].copy_from_slice(&self.size.to_le_bytes());
        b[16..24].copy_from_slice(&self.mtime.to_le_bytes());
        b[24..28].copy_from_slice(&self.kind.to_le_bytes());
        b[28..32].copy_from_slice(&self.name_len.to_le_bytes());
        b[32..].copy_from_slice(&self.name);
        b
    }
}

impl Default for DirEntry {
    fn default() -> DirEntry {
        DirEntry::new()
    }
}

/// Size of a [`DirEntry`].
pub const DIR_ENTRY_SIZE: usize = core::mem::size_of::<DirEntry>();

/// What `FileStat` returns, 48 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Stat {
    /// Persistent id.
    pub id: u64,
    /// Size in bytes.
    pub size: u64,
    /// Creation, ns since 1970.
    pub ctime: u64,
    /// Last change, ns since 1970.
    pub mtime: u64,
    /// `kind::FILE` or `kind::DIRECTORY`.
    pub kind: u32,
    /// The volume's index (as `VolumeList` counts).
    pub volume: u32,
    /// Bit 0: the item is on a read-only volume.
    pub flags: u32,
    /// 0.
    pub reserved: u32,
}

impl Stat {
    /// The bytes copied to user memory.
    pub fn to_bytes(&self) -> [u8; STAT_SIZE] {
        let mut b = [0u8; STAT_SIZE];
        b[0..8].copy_from_slice(&self.id.to_le_bytes());
        b[8..16].copy_from_slice(&self.size.to_le_bytes());
        b[16..24].copy_from_slice(&self.ctime.to_le_bytes());
        b[24..32].copy_from_slice(&self.mtime.to_le_bytes());
        b[32..36].copy_from_slice(&self.kind.to_le_bytes());
        b[36..40].copy_from_slice(&self.volume.to_le_bytes());
        b[40..44].copy_from_slice(&self.flags.to_le_bytes());
        b
    }
}

/// Size of a [`Stat`].
pub const STAT_SIZE: usize = core::mem::size_of::<Stat>();

/// What `VolumeList` returns, 96 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct VolumeInfo {
    /// Index (stable while the system runs).
    pub index: u32,
    /// Bit 0: read-only.
    pub flags: u32,
    /// `volume_kind::*`.
    pub kind: u32,
    /// Valid bytes of `name`.
    pub name_len: u32,
    /// Size in bytes.
    pub total: u64,
    /// Free bytes.
    pub free: u64,
    /// The volume name (UTF-8).
    pub name: [u8; 64],
}

impl VolumeInfo {
    /// An empty record.
    pub const fn new() -> VolumeInfo {
        VolumeInfo {
            index: 0,
            flags: 0,
            kind: 0,
            name_len: 0,
            total: 0,
            free: 0,
            name: [0; 64],
        }
    }

    /// The name.
    pub fn name(&self) -> &str {
        let n = (self.name_len as usize).min(64);
        core::str::from_utf8(&self.name[..n]).unwrap_or("?")
    }

    /// The bytes copied to user memory.
    pub fn to_bytes(&self) -> [u8; VOLUME_INFO_SIZE] {
        let mut b = [0u8; VOLUME_INFO_SIZE];
        b[0..4].copy_from_slice(&self.index.to_le_bytes());
        b[4..8].copy_from_slice(&self.flags.to_le_bytes());
        b[8..12].copy_from_slice(&self.kind.to_le_bytes());
        b[12..16].copy_from_slice(&self.name_len.to_le_bytes());
        b[16..24].copy_from_slice(&self.total.to_le_bytes());
        b[24..32].copy_from_slice(&self.free.to_le_bytes());
        b[32..].copy_from_slice(&self.name);
        b
    }
}

impl Default for VolumeInfo {
    fn default() -> VolumeInfo {
        VolumeInfo::new()
    }
}

/// Size of a [`VolumeInfo`].
pub const VOLUME_INFO_SIZE: usize = core::mem::size_of::<VolumeInfo>();

/// `VolumeInfo::kind`.
pub mod volume_kind {
    /// eHFS.
    pub const EHFS: u32 = 1;
    /// FAT (read only).
    pub const FAT: u32 = 2;
    /// The initial RAM disk (read only).
    pub const INITRD: u32 = 3;
    /// The kernel's memory file system (`Tmp`).
    pub const TMP: u32 = 4;
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
    assert!(DIR_ENTRY_SIZE == 288);
    assert!(STAT_SIZE == 48);
    assert!(VOLUME_INFO_SIZE == 96);
    assert!(MESSAGE_SIZE == 144);
    assert!(core::mem::offset_of!(Message, data) == 16);
    assert!(core::mem::offset_of!(Message, handle_slots) == 128);
};

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
        assert_eq!(Syscall::HandleClose.raw(), 7);
        assert_eq!(Syscall::HandleDuplicate.raw(), 8);
        assert_eq!(Syscall::PortCreate.raw(), 9);
        assert_eq!(Syscall::PortSend.raw(), 10);
        assert_eq!(Syscall::PortReceive.raw(), 11);
        assert_eq!(Syscall::MemoryCreate.raw(), 12);
        assert_eq!(Syscall::MemoryMap.raw(), 13);
        assert_eq!(Syscall::MemoryUnmap.raw(), 14);
        assert_eq!(Syscall::InputAttach.raw(), 15);
        assert_eq!(Syscall::DirOpen.raw(), 16);
        assert_eq!(Syscall::DirRead.raw(), 17);
        assert_eq!(Syscall::DirRemove.raw(), 18);
        assert_eq!(Syscall::DirRename.raw(), 19);
        assert_eq!(Syscall::FileRead.raw(), 20);
        assert_eq!(Syscall::FileWrite.raw(), 21);
        assert_eq!(Syscall::FileStat.raw(), 22);
        assert_eq!(Syscall::FileSetSize.raw(), 23);
        assert_eq!(Syscall::VolumeList.raw(), 24);
        assert_eq!(Syscall::VolumeOpen.raw(), 25);
        assert_eq!(Syscall::VolumeOpenById.raw(), 26);
        for call in Syscall::ALL {
            assert_eq!(Syscall::from_raw(call.raw()), Some(call));
        }
        assert_eq!(Syscall::from_raw(REMOVED_DEBUG_WRITE), None);
        assert_eq!(Syscall::ProcessSpawn.raw(), 27);
        assert_eq!(Syscall::ProcessWait.raw(), 28);
        assert_eq!(Syscall::ProcessKill.raw(), 29);
        assert_eq!(Syscall::ProcessSelf.raw(), 30);
        assert_eq!(Syscall::SysInfo.raw(), 31);
        assert_eq!(Syscall::SystemPower.raw(), 32);
        assert_eq!((power::SHUTDOWN, power::REBOOT), (0, 1));
        assert_eq!((spawn::FORCE_LINUX, spawn::FORCE_STELA), (1 << 32, 1 << 33));
        assert_eq!(&note::BYTES[12..17], note::NAME);
        assert_eq!(
            u32::from_le_bytes(note::BYTES[8..12].try_into().unwrap()),
            note::ABI
        );
        assert_eq!(
            u32::from_le_bytes(note::BYTES[20..24].try_into().unwrap()),
            note::VERSION
        );
        assert_eq!(Syscall::TtyCreate.raw(), 33);
        assert_eq!(Syscall::TtyRead.raw(), 34);
        assert_eq!(Syscall::TtyWrite.raw(), 35);
        assert_eq!(Syscall::HandleKind.raw(), 36);
        assert_eq!(tty::MASTER, 1);
        assert_eq!((object_kind::PORT, object_kind::TTY_MASTER), (1, 7));
        assert_eq!(object_kind::FRAMEBUFFER, 8);
        assert_eq!(sysinfo::FRAMEBUFFER, 4);
        let fb = sysinfo::FramebufferInfo {
            width: 2560,
            height: 1600,
            pitch: 10240,
            format: sysinfo::FORMAT_BGRX,
            size: 10240 * 1600,
            scale: 1,
        };
        assert_eq!(
            sysinfo::FramebufferInfo::from_bytes(&fb.to_bytes()),
            Some(fb)
        );
        assert_eq!(&fb.to_bytes()[..4], &2560u32.to_le_bytes());
        assert_eq!(Syscall::FileGetInfo.raw(), 37);
        assert_eq!(Syscall::FileSetInfo.raw(), 38);
        assert_eq!(FINDER_INFO_SIZE, 32);
        assert_eq!(Syscall::from_raw(999), None);
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
        assert_eq!(Error::NotFound.raw(), 14);
        assert_eq!(Error::Exists.raw(), 15);
        assert_eq!(Error::NotDir.raw(), 16);
        assert_eq!(Error::IsDir.raw(), 17);
        assert_eq!(Error::NotEmpty.raw(), 18);
        assert_eq!(Error::ReadOnly.raw(), 19);
        assert_eq!(Error::NoSpace.raw(), 20);
        assert_eq!(Error::IoError.raw(), 21);
        for e in Error::ALL {
            assert_eq!(Error::from_raw(e.raw()), Some(e));
            assert!(!e.name().is_empty());
        }
        assert_eq!(Error::from_raw(0), None);
        assert_eq!(Error::from_raw(27), None);
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
        assert_eq!(Rights::KILL.0, 256);
        let r = Rights::SEND | Rights::RECEIVE;
        assert!(r.contains(Rights::SEND));
        assert!(!r.contains(Rights::SEND | Rights::MAP));
        assert_eq!(r.without(Rights::SEND), Rights::RECEIVE);
        assert_eq!(Rights::from_raw(0x400), None);
        assert_eq!(Rights::from_raw(0x3FF), Some(Rights::ALL));
        assert_eq!(Rights::MANAGE.raw(), 0x200);
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

    #[test]
    fn file_structures() {
        let mut e = DirEntry::new();
        e.id = 17;
        e.kind = kind::DIRECTORY;
        e.name[..4].copy_from_slice(b"docs");
        e.name_len = 4;
        let b = e.to_bytes();
        assert_eq!(&b[0..8], &17u64.to_le_bytes());
        assert_eq!(&b[24..28], &2u32.to_le_bytes());
        assert_eq!(&b[32..36], b"docs");
        assert_eq!(e.name(), "docs");
        assert_eq!(core::mem::offset_of!(DirEntry, name), 32);
        assert_eq!(core::mem::offset_of!(Stat, kind), 32);
        assert_eq!(core::mem::offset_of!(VolumeInfo, name), 32);
        assert_eq!(bootstrap::CWD.0, 0x0100_0003);
        assert_eq!(bootstrap::PROGRAM.0, 0x0100_0004);
        assert_eq!(bootstrap::STDIN.0, 0x0100_0005);
        assert_eq!((bootstrap::COUNT, bootstrap::GIVEN), (7, 6));
        assert_eq!(open::ALL, 0x3F);
    }

    #[test]
    fn sysinfo_records() {
        let mut name = [0u8; 24];
        name[..5].copy_from_slice(b"shell");
        let p = sysinfo::ProcessInfo {
            pid: 3,
            threads: 1,
            state: 1,
            exit_code: -1,
            name,
        };
        let b = p.to_bytes();
        assert_eq!(&b[0..8], &3u64.to_le_bytes());
        assert_eq!(sysinfo::ProcessInfo::from_bytes(&b), Some(p));
        assert_eq!(p.name(), "shell");
        let m = sysinfo::MemInfo {
            total_frames: 1,
            free_frames: 2,
            heap_size: 3,
            heap_used: 4,
        };
        assert_eq!(sysinfo::MemInfo::from_bytes(&m.to_bytes()), Some(m));
        assert_eq!(tag::BOOTSTRAP_ARGS, 3);
        assert_eq!(EXIT_KILLED, 137);
    }

    #[test]
    fn cmdline_options() {
        use sysinfo::{cmdline_flag, cmdline_option};
        let c = b"textmode nousb classic=off serial=0 serial";
        assert!(cmdline_flag(c, "textmode"));
        assert!(cmdline_flag(c, "NOUSB"));
        assert!(!cmdline_flag(c, "classic"));
        assert!(!cmdline_flag(c, "nosmp"));
        assert!(cmdline_flag(c, "serial"));
        assert!(!cmdline_flag(b"serial serial=no", "serial"));
        assert_eq!(cmdline_option(c, "classic"), Some(&b"off"[..]));
        assert_eq!(cmdline_option(c, "textmode"), Some(&b""[..]));
        assert_eq!(cmdline_option(c, "text"), None);
        assert_eq!(cmdline_option(b"", "text"), None);
        assert_eq!(sysinfo::CMDLINE, 5);
        assert_eq!(sysinfo::TIME, 6);
        assert_eq!(sysinfo::CPUS, 7);
        assert_eq!(sysinfo::PROCESS_CPUS, 8);
    }

    #[test]
    fn network_abi() {
        assert_eq!(Syscall::SocketCreate.raw(), 39);
        assert_eq!(Syscall::NetInfo.raw(), 46);
        assert_eq!(Error::AddressInUse.raw(), 25);
        assert_eq!(Error::Busy.raw(), 26);
        assert_eq!(Syscall::AudioOpen.raw(), 47);
        assert_eq!(Syscall::AudioClose.raw(), 48);
        assert_eq!(audio::MEMORY_SIZE % 4096, 0);
        let v = net::pack([10, 0, 2, 2], 8080);
        assert_eq!(net::unpack(v), ([10, 0, 2, 2], 8080));
        assert_eq!(v, 0x0A00_0202_1F90);
        let info = net::NetInfo {
            driver: *b"e1000\0\0\0\0\0\0\0\0\0\0\0",
            mac: [0x52, 0x54, 0, 0x12, 0x34, 0x56],
            link: true,
            configured: true,
            ip: [10, 0, 2, 15],
            mask: [255, 255, 255, 0],
            gateway: [10, 0, 2, 2],
            dns: [10, 0, 2, 3],
        };
        assert_eq!(net::NetInfo::from_bytes(&info.to_bytes()), Some(info));
        assert_eq!(info.driver_name(), "e1000");
    }

    #[test]
    fn wait_abi() {
        assert_eq!(Syscall::WaitSetCreate.raw(), 49);
        assert_eq!(Syscall::WaitSetAdd.raw(), 50);
        assert_eq!(Syscall::WaitSetRemove.raw(), 51);
        assert_eq!(Syscall::WaitSetWait.raw(), 52);
        assert_eq!(Syscall::FilePath.raw(), 53);
        assert_eq!(Syscall::PipeCreate.raw(), 54);
        assert_eq!(Syscall::PipeWrite.raw(), 56);
        assert_eq!(Syscall::TtySetSize.raw(), 57);
        assert_eq!((object_kind::PIPE_READ, object_kind::PIPE_WRITE), (13, 14));
        assert_eq!(PIPE_BUFFER, 65536);
        assert_eq!(object_kind::WAIT_SET, 11);
        assert_eq!(wait::ALL, 31);
        let r = wait::Ready {
            key: 0x1122_3344_5566_7788,
            events: wait::READABLE | wait::CLOSED,
        };
        let b = r.to_bytes();
        assert_eq!(b[0], 0x88);
        assert_eq!(b[8], 9);
        assert_eq!(&b[12..], &[0, 0, 0, 0]);
        assert_eq!(wait::Ready::from_bytes(&b), r);
    }

    #[test]
    fn capabilities() {
        use capability::*;
        assert_eq!((POWER, INPUT, NETWORK, INIT, MAX), (1, 2, 3, 4, 4));
        assert!(is_valid(POWER) && is_valid(INIT));
        assert!(!is_valid(0) && !is_valid(5));
        assert_eq!((tag::CAPABILITIES, tag::POWER_REQUEST), (7, 8));
        assert_eq!(object_kind::CAPABILITY, 12);
    }
}
