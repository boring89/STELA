// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a Linux process has beyond a STELA one: its descriptor table, its
//! working directory (a Linux path) and its memory (the `brk` area).

use super::fd::FdTable;
use crate::object::tty::Tty;
use crate::vfs::Node;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::AtomicU64;
use frame::sync::SpinLock;

/// An open file: what a descriptor names. Descriptors made by `dup` share
/// one (so they share the offset, as in Linux).
pub enum OpenFile {
    /// A terminal.
    Tty(Arc<Tty>),
    /// `/dev/null`.
    Null,
    /// `/dev/zero`.
    Zero,
    /// A file on a volume.
    File {
        /// The file.
        node: Node,
        /// Offset and flags.
        state: SpinLock<FileState>,
    },
    /// A folder (for `getdents64`): a volume's folder, the synthetic root
    /// or `/dev`.
    Dir {
        /// The folder; `None` for the synthetic root and `/dev`.
        node: Option<Node>,
        /// Its Linux path (for `/` and `/dev` listings).
        path: String,
        /// How many entries `getdents64` has returned.
        cursor: SpinLock<u64>,
    },
    /// An `AF_INET` socket (`docs/net.md`).
    Socket(Arc<super::socket::LinuxSocket>),
    /// A pipe's reading end (P11) and whether reads do not wait
    /// (`O_NONBLOCK`).
    PipeRead(
        Arc<crate::object::pipe::ReadEnd>,
        core::sync::atomic::AtomicBool,
    ),
    /// A pipe's writing end.
    PipeWrite(
        Arc<crate::object::pipe::WriteEnd>,
        core::sync::atomic::AtomicBool,
    ),
    /// A generated, read-only file (`/etc/resolv.conf`): its bytes, made
    /// when opened, and the offset.
    Text {
        /// The contents.
        data: Vec<u8>,
        /// Where the next read starts.
        offset: SpinLock<u64>,
    },
}

/// The changing part of an open file.
pub struct FileState {
    /// Where the next read or write happens.
    pub offset: u64,
    /// `O_APPEND`: writes go to the end.
    pub append: bool,
    /// Opened for reading.
    pub readable: bool,
    /// Opened for writing.
    pub writable: bool,
    /// The `open` flags as given (for `F_GETFL`).
    pub flags: u64,
}

/// The `brk` area: from the end of the program up.
pub struct Memory {
    /// The page-aligned end of the program image, where the area starts.
    pub start: u64,
    /// The current break (what `brk` returns).
    pub current: u64,
    /// The end of the mapped pages (page aligned, ≥ `current`).
    pub mapped: u64,
}

/// A Linux process's state.
pub struct State {
    /// Descriptors.
    pub fds: SpinLock<FdTable<Arc<OpenFile>>>,
    /// The working directory, absolute and normalised (`/Data/docs`).
    pub cwd: SpinLock<String>,
    /// The `brk` area.
    pub memory: SpinLock<Memory>,
    /// The controlling terminal (`/dev/tty`).
    pub tty: Arc<Tty>,
    /// `set_tid_address` (kept, never used: no threads).
    pub tid_address: AtomicU64,
    /// Whether it was given the `NETWORK` capability (`socket` needs it;
    /// P11).
    pub network: bool,
    /// The volumes it sees under `/` (besides `tmp`): those of its `[3]`
    /// and `[4]` (P11).
    pub volumes: Vec<String>,
}

impl State {
    /// A new state: descriptors 0, 1 and 2 on `tty`, working directory
    /// `cwd`, the break at `image_end` (rounded up to a page).
    pub fn new(tty: Arc<Tty>, cwd: String, image_end: u64) -> State {
        State::with_streams(tty, None, None, cwd, image_end)
    }

    /// Like [`State::new`], with descriptor 0 on `stdin` and 1, 2 on
    /// `stdout` when given (P11: a shell pipeline's pipes or files).
    pub fn with_streams(
        tty: Arc<Tty>,
        stdin: Option<OpenFile>,
        stdout: Option<OpenFile>,
        cwd: String,
        image_end: u64,
    ) -> State {
        let mut fds = FdTable::new();
        let file = Arc::new(OpenFile::Tty(Arc::clone(&tty)));
        let input = stdin.map_or_else(|| Arc::clone(&file), Arc::new);
        let output = stdout.map_or_else(|| Arc::clone(&file), Arc::new);
        // An empty table has room for three.
        let _ = fds.insert(input);
        let _ = fds.insert(Arc::clone(&output));
        let _ = fds.insert(output);
        let start = image_end.next_multiple_of(4096);
        State {
            fds: SpinLock::new(fds),
            cwd: SpinLock::new(cwd),
            memory: SpinLock::new(Memory {
                start,
                current: start,
                mapped: start,
            }),
            tty,
            tid_address: AtomicU64::new(0),
            network: false,
            volumes: Vec::new(),
        }
    }

    /// Closes every descriptor (the process ended). The files are dropped
    /// after the lock is released.
    pub fn release(&self) {
        let files: Vec<Arc<OpenFile>> = self.fds.lock().drain();
        drop(files);
    }
}
