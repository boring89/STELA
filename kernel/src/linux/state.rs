// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a Linux process has beyond a STELA one: its descriptor table, its
//! working directory (a Linux path), its memory (the `brk` area), its
//! threads, signal actions, futex waiters and children (P12,
//! `docs/linux.md` §11–§12).

use super::fd::FdTable;
use super::signal::{Action, Info};
use crate::object::tty::Tty;
use crate::task::{ProcessId, ThreadId, WaitQueue};
use crate::vfs::Node;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
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
    /// A pseudo-terminal's master side (P12): the terminal, its number
    /// and whether reads do not wait (`O_NONBLOCK`).
    PtyMaster(Arc<Tty>, u32, AtomicBool),
    /// `/dev/urandom` (P12).
    Random,
    /// A generated, read-only file (`/etc/resolv.conf`, `/proc/...`): its
    /// bytes, made when opened, and the offset.
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

/// A thread's signal state.
pub struct ThreadSignals {
    /// Blocked signals.
    pub mask: u64,
    /// Pending signals.
    pub pending: u64,
    /// What each pending signal says (index = signal − 1).
    pub info: [Info; 64],
    /// The mask to restore after a `rt_sigsuspend` (the frame saves this
    /// one instead of the temporary).
    pub saved_mask: Option<u64>,
    /// A system call returned `EINTR` because of a signal: its number,
    /// for `SA_RESTART`.
    pub restart: Option<u64>,
    /// `sigaltstack`: base, size, flags (recorded, not used).
    pub altstack: (u64, u64, u32),
    /// The exception behind a pending fault signal (for the frame).
    pub trap: Option<super::signal::Trap>,
}

/// One Linux thread (a STELA thread of a Linux process).
pub struct LinuxThread {
    /// Its Linux thread id: the process id for the first thread, else a
    /// number from the same counter (so ids never collide with pids).
    pub tid: u64,
    /// The STELA thread (0 until it is spawned).
    pub thread: AtomicU64,
    /// `CLONE_CHILD_CLEARTID`/`set_tid_address`: written 0 and woken
    /// (futex) when the thread exits.
    pub clear_child_tid: AtomicU64,
    /// Signal mask and pending signals.
    pub signals: SpinLock<ThreadSignals>,
    /// `pending & !mask != 0`, readable without the lock (the scheduler
    /// checks it on every return to user mode).
    pub deliverable: AtomicBool,
}

impl LinuxThread {
    /// A new thread with signal mask `mask`.
    pub fn new(tid: u64, mask: u64) -> Arc<LinuxThread> {
        Arc::new(LinuxThread {
            tid,
            thread: AtomicU64::new(0),
            clear_child_tid: AtomicU64::new(0),
            signals: SpinLock::new(ThreadSignals {
                mask,
                pending: 0,
                info: [Info::default(); 64],
                saved_mask: None,
                restart: None,
                altstack: (0, 0, 2),
                trap: None,
            }),
            deliverable: AtomicBool::new(false),
        })
    }

    /// Recomputes `deliverable` (call with the lock held, after changing
    /// `mask` or `pending`).
    pub fn update(&self, s: &ThreadSignals) {
        self.deliverable
            .store(s.pending & !s.mask != 0, Ordering::Release);
    }

    /// Whether a signal waits for delivery, or the thread must check its
    /// interrupted system call.
    pub fn needs_attention(&self) -> bool {
        self.deliverable.load(Ordering::Acquire)
    }
}

/// A thread waiting in `FUTEX_WAIT`.
pub struct FutexWaiter {
    /// The user address.
    pub addr: u64,
    /// `FUTEX_WAIT_BITSET`'s mask (all ones for `FUTEX_WAIT`).
    pub bitset: u32,
    /// The waiting thread.
    pub thread: ThreadId,
}

/// A child process (made by `fork`/`clone`) and, once it ended, its
/// `wait4` status.
pub struct Child {
    /// Its process id.
    pub pid: ProcessId,
    /// Its process group (for `wait4(-pgid)`).
    pub pgid: u64,
    /// (exit code, killing signal) once it ended: a zombie until waited.
    pub ended: Option<(i64, Option<u32>)>,
}

/// An `ITIMER_REAL` timer (`alarm`, `setitimer`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Itimer {
    /// When it fires (`now_ns`), 0 = disarmed.
    pub deadline: u64,
    /// The period after that (0 = once).
    pub interval: u64,
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
    /// Whether it was given the `NETWORK` capability (`socket` needs it;
    /// P11).
    pub network: bool,
    /// The volumes it sees under `/` (besides `tmp`): those of its `[3]`
    /// and `[4]` (P11).
    pub volumes: Vec<String>,
    /// Its threads (P12).
    pub threads: SpinLock<Vec<Arc<LinuxThread>>>,
    /// Signal actions (index = signal − 1).
    pub actions: SpinLock<[Action; 64]>,
    /// Threads in `FUTEX_WAIT`.
    pub futex: SpinLock<Vec<FutexWaiter>>,
    /// The parent's process id (1 for processes started by STELA).
    pub parent: AtomicU64,
    /// Process group and session.
    pub pgid: AtomicU64,
    /// Session id.
    pub sid: AtomicU64,
    /// Children not yet waited for.
    pub children: SpinLock<Vec<Child>>,
    /// Threads in `wait4`/`waitid`.
    pub child_waiters: SpinLock<WaitQueue>,
    /// The signal that ended the process (0: none), for the parent's
    /// `wait4`.
    pub term_signal: AtomicU32,
    /// The code of the last thread that ended with `exit` (the process's
    /// exit code when that was the last thread).
    pub last_exit: AtomicI64,
    /// `umask`.
    pub umask: AtomicU32,
    /// `ITIMER_REAL`.
    pub itimer: SpinLock<Itimer>,
    /// The program's Linux path (`/proc/self/exe`).
    pub exe: SpinLock<String>,
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
            network: false,
            volumes: Vec::new(),
            threads: SpinLock::new(Vec::new()),
            actions: SpinLock::new([Action::default(); 64]),
            futex: SpinLock::new(Vec::new()),
            parent: AtomicU64::new(1),
            pgid: AtomicU64::new(0),
            sid: AtomicU64::new(0),
            children: SpinLock::new(Vec::new()),
            child_waiters: SpinLock::new(WaitQueue::new()),
            term_signal: AtomicU32::new(0),
            last_exit: AtomicI64::new(0),
            umask: AtomicU32::new(0o022),
            itimer: SpinLock::new(Itimer::default()),
            exe: SpinLock::new(String::new()),
        }
    }

    /// The state of a `fork` child of the process `parent`: the same
    /// descriptors (sharing the open files), working directory, break,
    /// terminal, volumes, signal actions, process group and session; no
    /// threads, children or timers.
    pub fn fork_copy(&self, parent: ProcessId) -> State {
        let fds = self.fds.lock().clone();
        let m = self.memory.lock();
        State {
            fds: SpinLock::new(fds),
            cwd: SpinLock::new(self.cwd.lock().clone()),
            memory: SpinLock::new(Memory {
                start: m.start,
                current: m.current,
                mapped: m.mapped,
            }),
            tty: Arc::clone(&self.tty),
            network: self.network,
            volumes: self.volumes.clone(),
            threads: SpinLock::new(Vec::new()),
            actions: SpinLock::new(*self.actions.lock()),
            futex: SpinLock::new(Vec::new()),
            parent: AtomicU64::new(parent),
            pgid: AtomicU64::new(self.pgid.load(Ordering::Relaxed)),
            sid: AtomicU64::new(self.sid.load(Ordering::Relaxed)),
            children: SpinLock::new(Vec::new()),
            child_waiters: SpinLock::new(WaitQueue::new()),
            term_signal: AtomicU32::new(0),
            last_exit: AtomicI64::new(0),
            umask: AtomicU32::new(self.umask.load(Ordering::Relaxed)),
            itimer: SpinLock::new(Itimer::default()),
            exe: SpinLock::new(self.exe.lock().clone()),
        }
    }

    /// The thread with Linux id `tid`.
    pub fn thread(&self, tid: u64) -> Option<Arc<LinuxThread>> {
        self.threads.lock().iter().find(|t| t.tid == tid).cloned()
    }

    /// The process's action for `sig`.
    pub fn action(&self, sig: u32) -> Action {
        self.actions.lock()[(sig - 1) as usize]
    }

    /// Closes every descriptor (the process ended). The files are dropped
    /// after the lock is released.
    pub fn release(&self) {
        let files: Vec<Arc<OpenFile>> = self.fds.lock().drain();
        drop(files);
    }
}
