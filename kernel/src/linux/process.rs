// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Linux process model (P12, `docs/linux.md` §11): threads (`clone`
//! with `CLONE_THREAD` makes a STELA thread of the same process), `fork`
//! (a new process with a copy of the address space), `execve` (a new
//! program in the same process), `wait4`/`waitid` and the two exits.
//!
//! ```text
//!  clone(CLONE_VM|CLONE_THREAD|…) ─► spawn_user_regs(same process)
//!  fork / vfork / clone(SIGCHLD)  ─► clone_user() ─► create_fork ─► child
//!                                     parent.children += child
//!  child ends ─► process_ended ─► parent: child's status, wake wait4,
//!                                         SIGCHLD
//!  execve ─► load into a new AddressSpace ─► other threads killed
//!          ─► swap_user ─► registers = entry, new stack
//! ```

use super::errno::{E2BIG, EAGAIN, ECHILD, EFAULT, EINTR, EINVAL, ENOEXEC, ENOMEM};
use super::files;
use super::signal::{self, Info, sig};
use super::signals;
use super::state::{Child, LinuxThread};
use super::sys::{LinuxResult, read_u64, write_user};
use crate::program;
use crate::task::{self, Process, ProcessId};
use abi::Handle;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use elfload::{Elf, Layout};
use frame::mm::user::AddressSpace;
use frame::syscall::SyscallFrame;
use frame::trap::UserRegs;

const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
const CLONE_FILES: u64 = 0x400;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_VFORK: u64 = 0x4000;
const CLONE_THREAD: u64 = 0x1_0000;
const CLONE_SYSVSEM: u64 = 0x4_0000;
const CLONE_SETTLS: u64 = 0x8_0000;
const CLONE_PARENT_SETTID: u64 = 0x10_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x20_0000;
const CLONE_DETACHED: u64 = 0x40_0000;
const CLONE_CHILD_SETTID: u64 = 0x100_0000;

/// What a thread `clone` must have, and may have (musl's
/// `pthread_create`).
const THREAD_NEEDS: u64 = CLONE_VM | CLONE_SIGHAND | CLONE_THREAD;
const THREAD_FLAGS: u64 = CLONE_VM
    | CLONE_FS
    | CLONE_FILES
    | CLONE_SIGHAND
    | CLONE_THREAD
    | CLONE_SYSVSEM
    | CLONE_SETTLS
    | CLONE_PARENT_SETTID
    | CLONE_CHILD_CLEARTID
    | CLONE_DETACHED
    | CLONE_CHILD_SETTID;
/// What a process `clone` may have (`fork`, musl's `posix_spawn` with
/// `CLONE_VM|CLONE_VFORK`: the memory is copied, which the child cannot
/// tell apart because it only execs or exits), besides the exit signal in
/// the low byte.
const FORK_FLAGS: u64 = CLONE_VM
    | CLONE_FS
    | CLONE_FILES
    | CLONE_VFORK
    | CLONE_SETTLS
    | CLONE_PARENT_SETTID
    | CLONE_CHILD_CLEARTID
    | CLONE_CHILD_SETTID
    | 0xFF;

/// `clone(flags, stack, ptid, ctid, tls)` (x86-64 argument order).
pub fn clone(
    p: &Arc<Process>,
    frame: &SyscallFrame,
    flags: u64,
    stack: u64,
    ptid: u64,
    ctid: u64,
    tls: u64,
) -> LinuxResult {
    if flags & CLONE_THREAD != 0 {
        if flags & THREAD_NEEDS != THREAD_NEEDS || flags & !THREAD_FLAGS != 0 {
            return Err(EINVAL);
        }
        return new_thread(p, frame, flags, stack, ptid, ctid, tls);
    }
    if flags & !FORK_FLAGS != 0 {
        return Err(EINVAL);
    }
    fork(p, frame, flags, stack, ptid, ctid, tls)
}

/// `clone3(args, size)`: `struct clone_args` (flags, pidfd, child_tid,
/// parent_tid, exit_signal, stack, stack_size, tls, ...).
pub fn clone3(p: &Arc<Process>, frame: &SyscallFrame, args: u64, size: u64) -> LinuxResult {
    if size < 64 {
        return Err(EINVAL);
    }
    let word = |i: u64| read_u64(p, args + i * 8);
    let flags = word(0)?;
    let ctid = word(2)?;
    let ptid = word(3)?;
    let exit_signal = word(4)?;
    let stack = word(5)?;
    let stack_size = word(6)?;
    let tls = word(7)?;
    if flags & 0xFF != 0 || exit_signal > 64 {
        return Err(EINVAL);
    }
    let top = if stack != 0 { stack + stack_size } else { 0 };
    clone(p, frame, flags | exit_signal, top, ptid, ctid, tls)
}

/// The running thread's registers as a child starts with them: the same,
/// RAX = 0 (the child's return value), RSP = `stack` when given.
fn child_regs(frame: &SyscallFrame, stack: u64) -> UserRegs {
    let mut regs = frame.regs();
    regs.rax = 0;
    if stack != 0 {
        regs.rsp = stack;
    }
    regs
}

#[allow(clippy::too_many_arguments)]
fn new_thread(
    p: &Arc<Process>,
    frame: &SyscallFrame,
    flags: u64,
    stack: u64,
    ptid: u64,
    ctid: u64,
    tls: u64,
) -> LinuxResult {
    let linux = p.linux().ok_or(EINVAL)?;
    let tid = task::process::new_id();
    let mask = task::current_linux().map_or(0, |t| t.signals.lock().mask);
    let t = LinuxThread::new(tid, mask);
    if flags & CLONE_CHILD_CLEARTID != 0 {
        t.clear_child_tid.store(ctid, Ordering::Relaxed);
    }
    if flags & CLONE_PARENT_SETTID != 0 {
        write_user(p, ptid, &(tid as u32).to_le_bytes())?;
    }
    if flags & CLONE_CHILD_SETTID != 0 {
        write_user(p, ctid, &(tid as u32).to_le_bytes())?;
    }
    let fs = if flags & CLONE_SETTLS != 0 {
        tls
    } else {
        frame::task::current().fs_base()
    };
    linux.threads.lock().push(Arc::clone(&t));
    if task::spawn_user_regs(p, child_regs(frame, stack), fs, Some(Arc::clone(&t))).is_err() {
        linux.threads.lock().retain(|x| !Arc::ptr_eq(x, &t));
        return Err(EAGAIN);
    }
    Ok(tid)
}

/// `fork`, `vfork` and process `clone`s: a new process with a copy of the
/// address space, the same open files (shared offsets), working
/// directory, signal actions, terminal; one thread, continuing from the
/// call with RAX = 0. `vfork` does not wait for the child (the memory is a
/// copy, so it need not).
#[allow(clippy::too_many_arguments)]
pub fn fork(
    p: &Arc<Process>,
    frame: &SyscallFrame,
    flags: u64,
    stack: u64,
    ptid: u64,
    ctid: u64,
    tls: u64,
) -> LinuxResult {
    let linux = p.linux().ok_or(EINVAL)?;
    let space = p.space().clone_user().map_err(|_| ENOMEM)?;
    let maps = p.maps().lock().clone();
    let state = linux.fork_copy(p.id());
    let initial: task::process::Initial = {
        let handles = p.handles().lock();
        core::array::from_fn(|i| handles.get(Handle::new(i as u32 + 1, 1)).ok().cloned())
    };
    let child =
        task::process::create_fork(p.name(), initial, state, space, maps).map_err(|_| ENOMEM)?;
    let Some(child_linux) = child.linux() else {
        return Err(ENOMEM);
    };
    let pid = child.id();
    let mask = task::current_linux().map_or(0, |t| t.signals.lock().mask);
    let t = LinuxThread::new(pid, mask);
    if flags & CLONE_CHILD_CLEARTID != 0 {
        t.clear_child_tid.store(ctid, Ordering::Relaxed);
    }
    if flags & CLONE_CHILD_SETTID != 0 {
        child
            .space()
            .write(ctid, &(pid as u32).to_le_bytes())
            .map_err(|_| EFAULT)?;
    }
    if flags & CLONE_PARENT_SETTID != 0 {
        write_user(p, ptid, &(pid as u32).to_le_bytes())?;
    }
    child_linux.threads.lock().push(Arc::clone(&t));
    child_linux.tty.add_user(pid);
    linux.children.lock().push(Child {
        pid,
        pgid: child_linux.pgid.load(Ordering::Relaxed),
        ended: None,
    });
    let fs = if flags & CLONE_SETTLS != 0 {
        tls
    } else {
        frame::task::current().fs_base()
    };
    if task::spawn_user_regs(&child, child_regs(frame, stack), fs, Some(t)).is_err() {
        linux.children.lock().retain(|c| c.pid != pid);
        return Err(EAGAIN);
    }
    Ok(pid)
}

/// The end of a Linux process (its last thread was freed): its parent
/// learns the status (a zombie until `wait4`), is woken and gets
/// `SIGCHLD`.
pub fn process_ended(process: &Process, code: i64) {
    let Some(linux) = process.linux() else {
        return;
    };
    let signo = match linux.term_signal.load(Ordering::Relaxed) {
        0 => None,
        s => Some(s),
    };
    let parent_id = linux.parent.load(Ordering::Relaxed);
    let Some(parent) = task::process::find(parent_id) else {
        return;
    };
    let Some(pl) = parent.linux() else {
        return;
    };
    let action = pl.action(sig::SIGCHLD);
    // SIGCHLD ignored, or SA_NOCLDWAIT: no zombie.
    let reap = action.handler == signal::SIG_IGN || action.flags & 2 != 0;
    let waiters = {
        let mut children = pl.children.lock();
        if reap {
            children.retain(|c| c.pid != process.id());
        } else if let Some(c) = children.iter_mut().find(|c| c.pid == process.id()) {
            c.ended = Some((code, signo));
        }
        pl.child_waiters.lock().drain()
    };
    for w in waiters {
        task::wake(w);
    }
    signals::send_process(
        &parent,
        Info {
            signo: sig::SIGCHLD,
            code: if signo.is_some() {
                signal::CLD_KILLED
            } else {
                signal::CLD_EXITED
            },
            pid: process.id() as u32,
            status: signo.map_or(code as i32 & 0xFF, |s| s as i32),
            addr: 0,
        },
    );
}

const WNOHANG: u64 = 1;
const WEXITED: u64 = 4;
const WNOWAIT: u64 = 0x0100_0000;

/// Which children a wait is for.
#[derive(Clone, Copy)]
enum Which {
    Any,
    Pid(ProcessId),
    Group(u64),
}

impl Which {
    fn matches(self, c: &Child) -> bool {
        match self {
            Which::Any => true,
            Which::Pid(pid) => c.pid == pid,
            Which::Group(g) => c.pgid == g,
        }
    }
}

/// Waits for a child that matches `which` to end: (pid, code, signal).
/// `None` with `WNOHANG` when none has ended yet.
fn wait_child(
    p: &Process,
    which: Which,
    options: u64,
    keep: bool,
) -> Result<Option<(ProcessId, i64, Option<u32>)>, i64> {
    let linux = p.linux().ok_or(ECHILD)?;
    let me = task::current_id();
    loop {
        {
            let mut children = linux.children.lock();
            if !children.iter().any(|c| which.matches(c)) {
                return Err(ECHILD);
            }
            if let Some(i) = children
                .iter()
                .position(|c| which.matches(c) && c.ended.is_some())
            {
                let c = if keep {
                    let c = &children[i];
                    Child {
                        pid: c.pid,
                        pgid: c.pgid,
                        ended: c.ended,
                    }
                } else {
                    children.remove(i)
                };
                let (code, signo) = c.ended.unwrap_or((0, None));
                return Ok(Some((c.pid, code, signo)));
            }
            if options & WNOHANG != 0 {
                return Ok(None);
            }
            linux.child_waiters.lock().add(me);
        }
        if task::current_killed() {
            linux.child_waiters.lock().remove(me);
            return Err(EINTR);
        }
        task::block_until(None);
        linux.child_waiters.lock().remove(me);
    }
}

/// `wait4(pid, status, options, rusage)`.
pub fn wait4(p: &Process, pid: i64, status: u64, options: u64, rusage: u64) -> LinuxResult {
    let which = match pid {
        -1 => Which::Any,
        0 => Which::Group(p.linux().map_or(0, |l| l.pgid.load(Ordering::Relaxed))),
        n if n < -1 => Which::Group(n.unsigned_abs()),
        n => Which::Pid(n as u64),
    };
    match wait_child(p, which, options, false)? {
        None => Ok(0),
        Some((child, code, signo)) => {
            if status != 0 {
                write_user(p, status, &signal::wait_status(code, signo).to_le_bytes())?;
            }
            if rusage != 0 {
                write_user(p, rusage, &[0u8; 144])?;
            }
            Ok(child)
        }
    }
}

/// `waitid(idtype, id, infop, options, rusage)`.
pub fn waitid(p: &Process, idtype: u64, id: u64, infop: u64, options: u64) -> LinuxResult {
    const P_ALL: u64 = 0;
    const P_PID: u64 = 1;
    const P_PGID: u64 = 2;
    if options & WEXITED == 0 {
        return Err(EINVAL);
    }
    let which = match idtype {
        P_ALL => Which::Any,
        P_PID => Which::Pid(id),
        P_PGID => Which::Group(id),
        _ => return Err(EINVAL),
    };
    let found = wait_child(p, which, options, options & WNOWAIT != 0)?;
    if infop != 0 {
        let info = match found {
            None => [0u8; 128],
            Some((child, code, signo)) => Info {
                signo: sig::SIGCHLD,
                code: if signo.is_some() {
                    signal::CLD_KILLED
                } else {
                    signal::CLD_EXITED
                },
                pid: child as u32,
                status: signo.map_or(code as i32 & 0xFF, |s| s as i32),
                addr: 0,
            }
            .to_bytes(),
        };
        write_user(p, infop, &info)?;
    }
    Ok(0)
}

/// `exit(code)`: ends the calling thread only (the process ends with its
/// last thread, with this code if it is the last). The thread's
/// `clear_child_tid` word is zeroed and woken (`pthread_join`).
pub fn exit_thread(p: Arc<Process>, code: u64) -> ! {
    if let Some(linux) = p.linux() {
        linux.last_exit.store(code as i64 & 0xFF, Ordering::Relaxed);
        if let Some(t) = task::current_linux() {
            let addr = t.clear_child_tid.swap(0, Ordering::Relaxed);
            if addr != 0 && write_user(&p, addr, &0u32.to_le_bytes()).is_ok() {
                super::futex::wake(&p, addr, 1, u32::MAX);
            }
            linux.threads.lock().retain(|x| !Arc::ptr_eq(x, &t));
        }
    }
    drop(p);
    task::exit()
}

/// Largest argument or environment list `execve` takes (entries; bytes).
const ARG_MAX_ENTRIES: usize = 4096;
const ARG_MAX_BYTES: usize = 256 * 1024;

/// A NULL-terminated array of string pointers of the caller.
fn string_array(p: &Process, ptr: u64, total: &mut usize) -> Result<Vec<Vec<u8>>, i64> {
    let mut out = Vec::new();
    if ptr == 0 {
        return Ok(out);
    }
    for i in 0..ARG_MAX_ENTRIES as u64 {
        let s = read_u64(p, ptr + i * 8)?;
        if s == 0 {
            return Ok(out);
        }
        let text = files::user_string(p, s)?;
        *total += text.len() + 1;
        if *total > ARG_MAX_BYTES {
            return Err(E2BIG);
        }
        out.push(text.into_bytes());
    }
    Err(E2BIG)
}

/// The program image at a node (`ENOEXEC`-style errors).
fn read_image(node: &crate::vfs::Node) -> Result<Vec<u8>, i64> {
    program::read_program(&node.volume, node.id)
        .map(|(_, image)| image)
        .map_err(|_| super::errno::EACCES)
}

/// `execve(path, argv, envp)` and `execveat(dirfd, path, argv, envp,
/// flags)`: replaces the program of the calling process. On success the
/// call does not return to the old program: the registers are the new
/// program's start.
pub fn execve(
    p: &Arc<Process>,
    frame: &mut SyscallFrame,
    dirfd: u64,
    path_ptr: u64,
    argv_ptr: u64,
    envp_ptr: u64,
    flags: u64,
) -> LinuxResult {
    const AT_EMPTY_PATH: u64 = 0x1000;
    let linux = p.linux().ok_or(EINVAL)?;
    let path = files::exec_alias(p, &files::user_string(p, path_ptr)?);
    let mut total = 0;
    let mut argv = string_array(p, argv_ptr, &mut total)?;
    let env = string_array(p, envp_ptr, &mut total)?;
    let (node, mut exe) = if path.is_empty() && flags & AT_EMPTY_PATH != 0 {
        (files::fd_node(p, dirfd)?, String::new())
    } else {
        files::exec_file(p, dirfd, &path)?
    };
    let mut image = read_image(&node)?;
    if let Some((interp, arg)) = super::stack::shebang(&image) {
        let (inode, ipath) = files::exec_file(p, dirfd, &interp)?;
        image = read_image(&inode)?;
        let mut new_argv = Vec::new();
        new_argv.push(interp.into_bytes());
        if let Some(a) = arg {
            new_argv.push(a.into_bytes());
        }
        new_argv.push(path.clone().into_bytes());
        new_argv.extend(argv.into_iter().skip(1));
        argv = new_argv;
        exe = ipath;
    }
    let elf = Elf::parse_with(&image, Layout::Packed).map_err(|_| ENOEXEC)?;
    if program::personality_of(&elf) == task::Personality::Stela {
        // A STELA program cannot replace a Linux one in place.
        return Err(ENOEXEC);
    }
    let space = AddressSpace::new().map_err(|_| ENOMEM)?;
    program::load(&space, &elf).map_err(|e| match e {
        program::LoadError::OutOfMemory => ENOMEM,
        _ => ENOEXEC,
    })?;
    let stack = super::initial_stack(&elf, &argv, Some(&env), Some(linux));
    space.write(stack.rsp, &stack.bytes).map_err(|_| ENOMEM)?;

    // No way back from here: the other threads go, the new image comes.
    task::kill_other_threads(p);
    while p.live_threads() > 1 {
        task::yield_now();
    }
    p.space().swap_user(&space);
    drop(space);
    *p.maps().lock() =
        crate::object::regions::Regions::new(abi::layout::MAP_BASE, abi::layout::MAP_END);
    {
        let (_, image_end) = elf.image_span();
        let start = image_end.next_multiple_of(4096);
        let mut m = linux.memory.lock();
        m.start = start;
        m.current = start;
        m.mapped = start;
    }
    let closed = linux.fds.lock().take_cloexec();
    drop(closed);
    for a in linux.actions.lock().iter_mut() {
        if a.is_handler() {
            *a = signal::Action::default();
        }
    }
    linux.futex.lock().clear();
    // The calling thread becomes the process's only thread, with the
    // process id as its thread id (as Linux does).
    let old = task::current_linux();
    let (mask, pending) = old.as_ref().map_or((0, 0), |t| {
        let s = t.signals.lock();
        (s.mask, s.pending)
    });
    let t = LinuxThread::new(p.id(), mask);
    {
        let mut s = t.signals.lock();
        s.pending = pending;
        t.update(&s);
    }
    t.thread.store(task::current_id(), Ordering::Relaxed);
    *linux.threads.lock() = alloc::vec![Arc::clone(&t)];
    task::set_current_linux(t);
    *linux.exe.lock() = exe;
    let _ = frame::task::set_fs_base(0);
    frame::task::reset_fpu();
    let regs = UserRegs {
        rip: elf.entry(),
        rsp: stack.rsp,
        rflags: 0x202,
        ..UserRegs::default()
    };
    if frame.set_regs(&regs).is_err() {
        return Err(ENOEXEC);
    }
    Ok(0)
}
