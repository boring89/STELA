// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Linux signals in the kernel (`docs/linux.md` §12): sending (a pending
//! bit on one thread, which is woken), delivery on the way back to user
//! mode (a frame on the user stack, `signal.rs`), `rt_sigreturn`, the
//! signal system calls and the `ITIMER_REAL` timer thread.
//!
//! ```text
//!  kill / tkill / tty ^C / fault / child ended / alarm
//!        │
//!        ▼ send_process / send_thread: pending |= bit, wake the thread
//!  thread blocked in a system call ── current_killed() ──► gives up
//!        │                              (EINTR, or restart: SA_RESTART)
//!        ▼
//!  user_return ── deliver ── ignored ──────────► nothing (restart)
//!                    ├── default: terminate ───► exit_process(128 + sig)
//!                    └── handler ──────────────► rt_sigframe, RIP = handler
//!  handler returns ── restorer ── rt_sigreturn ─► registers, mask, FPU back
//! ```

use super::errno::{EAGAIN, EFAULT, EINTR, EINVAL, ESRCH};
use super::nr;
use super::signal::{self, Action, Info, SA_NODEFER, SA_RESETHAND, SA_RESTART, sig};
use super::state::{Itimer, LinuxThread};
use super::sys::{LinuxResult, read_u64, read_user, write_user};
use crate::task::{self, Process, ProcessId};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use frame::sync::SpinLock;
use frame::syscall::{SyscallFrame, UserFrame};

/// Queues `info.signo` on thread `t` and wakes it.
pub fn send_thread(t: &LinuxThread, info: Info) {
    {
        let mut s = t.signals.lock();
        s.pending |= signal::bit(info.signo);
        s.info[(info.signo - 1) as usize] = info;
        t.update(&s);
    }
    task::wake(t.thread.load(Ordering::Relaxed));
}

/// Sends `info.signo` to `process`: to the first thread that does not
/// block it (else the first thread). Ignored signals that would be taken
/// at once are dropped. A process without the Linux personality is
/// killed with exit code 128 + signal (unless the signal is ignored by
/// default). Returns whether the process exists.
pub fn send_process(process: &Arc<Process>, info: Info) {
    let sig = info.signo;
    let Some(linux) = process.linux() else {
        if signal::default_action(sig) == signal::DefaultAction::Terminate {
            task::kill(process, 128 + i64::from(sig));
        }
        return;
    };
    let threads: Vec<Arc<LinuxThread>> = linux.threads.lock().clone();
    let Some(first) = threads.first() else {
        return;
    };
    let target = threads
        .iter()
        .find(|t| t.signals.lock().mask & signal::bit(sig) == 0);
    let action = linux.action(sig);
    match target {
        Some(t) if action.ignores(sig) => {
            // Taken at once and ignored: nothing happens.
            let _ = t;
        }
        Some(t) => send_thread(t, info),
        None => send_thread(first, info),
    }
}

/// `Info` for a signal sent by process `pid` with `kill`.
fn user_info(signo: u32, pid: ProcessId) -> Info {
    Info {
        signo,
        code: signal::SI_USER,
        pid: pid as u32,
        ..Info::default()
    }
}

/// A signal from the kernel (`SIGINT` from a terminal, `SIGALRM`,
/// `SIGPIPE`, `SIGWINCH`) to process `pid`.
pub fn send_kernel(pid: ProcessId, signo: u32) {
    if let Some(process) = task::process::find(pid) {
        send_process(
            &process,
            Info {
                signo,
                code: signal::SI_KERNEL,
                ..Info::default()
            },
        );
    }
}

/// `SIGPIPE` to the calling thread (a write to a pipe nobody reads).
pub fn sigpipe() {
    if let Some(t) = task::current_linux() {
        send_thread(
            &t,
            Info {
                signo: sig::SIGPIPE,
                code: signal::SI_KERNEL,
                ..Info::default()
            },
        );
    }
}

/// Whether system call `number` is restarted after a handler with
/// `SA_RESTART` (signal(7): never for sleeps, waits on many descriptors
/// and the signal waits).
fn restartable(number: u64) -> bool {
    !matches!(
        number,
        nr::NANOSLEEP
            | nr::CLOCK_NANOSLEEP
            | nr::PAUSE
            | nr::RT_SIGSUSPEND
            | nr::POLL
            | nr::PPOLL
            | nr::SELECT
            | nr::PSELECT6
    )
}

/// System calls that may block and so be interrupted by a signal.
pub fn blocking(number: u64) -> bool {
    matches!(
        number,
        nr::READ
            | nr::READV
            | nr::WRITE
            | nr::WRITEV
            | nr::WAIT4
            | nr::WAITID
            | nr::FUTEX
            | nr::POLL
            | nr::PPOLL
            | nr::SELECT
            | nr::PSELECT6
            | nr::ACCEPT
            | nr::ACCEPT4
            | nr::CONNECT
            | nr::RECVFROM
            | nr::RECVMSG
            | nr::SENDTO
            | nr::SENDMSG
            | nr::NANOSLEEP
            | nr::CLOCK_NANOSLEEP
            | nr::PAUSE
            | nr::RT_SIGSUSPEND
    )
}

/// After a blocking call failed while a signal waits: it becomes `EINTR`
/// and the call is remembered for `SA_RESTART` (`deliver` decides).
pub fn interrupted(number: u64, result: LinuxResult) -> LinuxResult {
    // A wait the signal broke ends with EINTR (the Linux code) or
    // NotSupported (→ ENOSYS: what the kernel's waits return when the
    // thread must give up); other errors are the call's own (EPIPE with
    // its SIGPIPE is not restarted).
    if !matches!(result, Err(EINTR | super::errno::ENOSYS)) || !blocking(number) {
        return result;
    }
    let Some(t) = task::current_linux() else {
        return result;
    };
    if !t.needs_attention() || task::current_dying() {
        return result;
    }
    t.signals.lock().restart = Some(number);
    Err(EINTR)
}

/// What delivery decided.
enum Outcome {
    /// Back to user mode as the frame says.
    Return,
    /// The process ends with this signal.
    Terminate(u32),
}

/// Registers to re-execute system call `number` (RAX = number, RIP back
/// over the two-byte `syscall`).
fn restart_regs(regs: &mut frame::trap::UserRegs, number: u64) {
    regs.rax = number;
    regs.rip = regs.rip.wrapping_sub(2);
}

/// Called on the way back to user mode when `t` has a signal to take (or
/// an interrupted call to settle). Never returns when the signal ends the
/// process.
pub fn deliver(frame: &mut UserFrame<'_>, t: Arc<LinuxThread>) {
    let outcome = deliver_inner(frame, &t);
    drop(t);
    if let Outcome::Terminate(signo) = outcome {
        terminate(signo);
    }
}

/// Ends the calling process because of signal `signo`.
fn terminate(signo: u32) -> ! {
    if let Some(process) = task::current_process()
        && let Some(linux) = process.linux()
    {
        let _ = linux
            .term_signal
            .compare_exchange(0, signo, Ordering::Relaxed, Ordering::Relaxed);
    }
    task::exit_process(128 + i64::from(signo))
}

fn deliver_inner(frame: &mut UserFrame<'_>, t: &LinuxThread) -> Outcome {
    let Some(process) = task::current_process() else {
        return Outcome::Return;
    };
    let Some(linux) = process.linux() else {
        return Outcome::Return;
    };
    let mut restart = t.signals.lock().restart.take();
    loop {
        let taken = {
            let mut s = t.signals.lock();
            let ready = s.pending & !s.mask;
            let taken = signal::lowest(ready).map(|signo| {
                s.pending &= !signal::bit(signo);
                (signo, s.info[(signo - 1) as usize], s.trap.take())
            });
            t.update(&s);
            taken
        };
        let Some((signo, info, trap)) = taken else {
            // Nothing (left) to take: an interrupted call is restarted
            // (the signal was ignored, or taken by another thread).
            if let Some(number) = restart {
                let mut regs = frame.regs();
                restart_regs(&mut regs, number);
                let _ = frame.set_regs(&regs);
            }
            restore_suspend_mask(t);
            return Outcome::Return;
        };
        let action = linux.action(signo);
        if action.ignores(signo) {
            continue;
        }
        if !action.is_handler() {
            return Outcome::Terminate(signo);
        }
        let mut regs = frame.regs();
        if let Some(number) = restart.take()
            && action.flags & SA_RESTART != 0
            && restartable(number)
        {
            restart_regs(&mut regs, number);
        }
        return match push_frame(&process, t, frame, regs, signo, info, action, trap) {
            Ok(()) => Outcome::Return,
            // No room for the frame: Linux forces SIGSEGV.
            Err(()) => Outcome::Terminate(sig::SIGSEGV),
        };
    }
}

/// After `rt_sigsuspend`, when no handler ran: the old mask comes back.
fn restore_suspend_mask(t: &LinuxThread) {
    let mut s = t.signals.lock();
    if let Some(mask) = s.saved_mask.take() {
        s.mask = mask;
        t.update(&s);
    }
}

/// Builds the handler's frame on the user stack and points the registers
/// at the handler.
#[allow(clippy::too_many_arguments)]
fn push_frame(
    process: &Process,
    t: &LinuxThread,
    frame: &mut UserFrame<'_>,
    regs: frame::trap::UserRegs,
    signo: u32,
    info: Info,
    action: Action,
    trap: Option<signal::Trap>,
) -> Result<(), ()> {
    let restore_mask = {
        let mut s = t.signals.lock();
        let restore = s.saved_mask.take().unwrap_or(s.mask);
        let mut mask = s.mask | action.mask;
        if action.flags & SA_NODEFER == 0 {
            mask |= signal::bit(signo);
        }
        s.mask = mask & !signal::UNBLOCKABLE;
        t.update(&s);
        restore
    };
    if action.flags & SA_RESETHAND != 0
        && let Some(linux) = process.linux()
    {
        linux.actions.lock()[(signo - 1) as usize] = Action::default();
    }
    let fpu = frame::task::fpu_state();
    let alt = t.signals.lock().altstack;
    let (top, uc_stack) =
        signal::frame_stack(regs.rsp, alt, action.flags & signal::SA_ONSTACK != 0);
    let placed = signal::build(
        &regs,
        top,
        &info,
        restore_mask,
        &fpu,
        action.restorer,
        trap.unwrap_or_default(),
        uc_stack,
    );
    process
        .space()
        .write(placed.rsp, &placed.bytes)
        .map_err(|_| ())?;
    let mut new = regs;
    new.rsp = placed.rsp;
    new.rip = action.handler;
    new.rdi = u64::from(signo);
    new.rsi = placed.rsp + signal::INFO as u64;
    new.rdx = placed.rsp + signal::UC as u64;
    new.rax = 0;
    // The handler starts with string operations forwards (System V ABI).
    new.rflags &= !(1 << 10);
    frame.set_regs(&new).map_err(|_| ())
}

/// `rt_sigreturn`: the registers, mask and FPU state from the frame the
/// handler returned over (RSP − 8 = its start).
pub fn rt_sigreturn(p: &Process, frame: &mut SyscallFrame) -> LinuxResult {
    let start = frame.rsp().wrapping_sub(8);
    let bytes = read_user(p, start, signal::FRAME_SIZE).map_err(|_| EFAULT);
    let Some(restored) = bytes.ok().as_deref().and_then(signal::parse) else {
        return force_sigsegv(p);
    };
    if restored.fpstate != 0
        && let Ok(fp) = read_user(p, restored.fpstate, 512)
    {
        let mut area = [0u8; 512];
        area.copy_from_slice(&fp);
        frame::task::set_fpu_state(&area);
    }
    if restored.regs.sanitized().is_err() {
        return force_sigsegv(p);
    }
    if let Some(t) = task::current_linux() {
        let mut s = t.signals.lock();
        s.mask = restored.mask & !signal::UNBLOCKABLE;
        t.update(&s);
    }
    if frame.set_regs(&restored.regs).is_err() {
        return force_sigsegv(p);
    }
    // The dispatcher puts the result in RAX: give back the restored one.
    Ok(restored.regs.rax)
}

/// A frame `rt_sigreturn` cannot use: SIGSEGV with its default action and
/// unblocked (Linux's `force_sigsegv`), so the process ends on its way
/// back to user mode. Never ended here: this system call's own references
/// (the process) are still on the stack.
fn force_sigsegv(p: &Process) -> LinuxResult {
    if let Some(linux) = p.linux() {
        linux.actions.lock()[(sig::SIGSEGV - 1) as usize] = Action::default();
    }
    if let Some(t) = task::current_linux() {
        send_thread(
            &t,
            Info {
                signo: sig::SIGSEGV,
                code: signal::SI_KERNEL,
                ..Info::default()
            },
        );
        let mut s = t.signals.lock();
        s.mask &= !signal::bit(sig::SIGSEGV);
        t.update(&s);
    }
    Err(EFAULT)
}

/// The signal for an exception in ring 3 (vector) and its `si_code`.
fn fault_of(vector: u64, error: u64) -> Option<(u32, i32)> {
    match vector {
        14 => Some((
            sig::SIGSEGV,
            if error & 1 == 0 {
                signal::SEGV_MAPERR
            } else {
                signal::SEGV_ACCERR
            },
        )),
        13 => Some((sig::SIGSEGV, signal::SI_KERNEL)),
        // ILL_ILLOPN.
        6 => Some((sig::SIGILL, 2)),
        // FPE_INTDIV.
        0 => Some((sig::SIGFPE, 1)),
        16 | 19 => Some((sig::SIGFPE, 0)),
        1 | 3 => Some((sig::SIGTRAP, signal::SI_KERNEL)),
        12 | 17 => Some((sig::SIGBUS, signal::SI_KERNEL)),
        _ => None,
    }
}

/// A ring-3 exception of a Linux thread: if the thread has a handler for
/// its signal and does not block it, the handler runs (`Ok`). Otherwise
/// `Err(exit code)`: 128 + signal (the process ends).
pub fn fault_signal(
    trap: &mut frame::trap::TrapFrame,
    cr2: u64,
    t: Arc<LinuxThread>,
) -> Result<(), i64> {
    let Some((signo, code)) = fault_of(trap.vector, trap.error_code) else {
        return Err(128 + trap.vector as i64);
    };
    let handled = {
        let Some(process) = task::current_process() else {
            return Err(128 + i64::from(signo));
        };
        let Some(linux) = process.linux() else {
            return Err(128 + i64::from(signo));
        };
        let action = linux.action(signo);
        let blocked = t.signals.lock().mask & signal::bit(signo) != 0;
        let handled = action.is_handler() && !blocked;
        if !handled {
            let _ =
                linux
                    .term_signal
                    .compare_exchange(0, signo, Ordering::Relaxed, Ordering::Relaxed);
        }
        handled
    };
    if !handled {
        return Err(128 + i64::from(signo));
    }
    let addr = if trap.vector == 14 { cr2 } else { trap.rip };
    {
        let mut s = t.signals.lock();
        s.pending |= signal::bit(signo);
        s.info[(signo - 1) as usize] = Info {
            signo,
            code,
            addr,
            ..Info::default()
        };
        s.trap = Some(signal::Trap {
            trapno: trap.vector,
            err: trap.error_code,
            cr2,
        });
        t.update(&s);
    }
    deliver(&mut UserFrame::Trap(trap), t);
    Ok(())
}

/// `rt_sigaction(sig, act, oldact, sigsetsize)`.
pub fn rt_sigaction(p: &Process, signo: u64, act: u64, old: u64) -> LinuxResult {
    let linux = p.linux().ok_or(EINVAL)?;
    if !signal::valid(signo) {
        return Err(EINVAL);
    }
    let signo = signo as u32;
    if act != 0 && signal::bit(signo) & signal::UNBLOCKABLE != 0 {
        return Err(EINVAL);
    }
    let new = if act != 0 {
        let bytes = read_user(p, act, 32)?;
        let mut b = [0u8; 32];
        b.copy_from_slice(&bytes);
        let mut a = Action::from_bytes(&b);
        a.mask &= !signal::UNBLOCKABLE;
        Some(a)
    } else {
        None
    };
    let previous = {
        let mut actions = linux.actions.lock();
        let previous = actions[(signo - 1) as usize];
        if let Some(a) = new {
            actions[(signo - 1) as usize] = a;
        }
        previous
    };
    if old != 0 {
        write_user(p, old, &previous.to_bytes())?;
    }
    // A signal now ignored is dropped where it was pending (POSIX).
    if new.is_some_and(|a| a.ignores(signo)) {
        for t in linux.threads.lock().iter() {
            let mut s = t.signals.lock();
            s.pending &= !signal::bit(signo);
            t.update(&s);
        }
    }
    Ok(0)
}

/// `rt_sigprocmask(how, set, oldset, sigsetsize)`.
pub fn rt_sigprocmask(p: &Process, how: u64, set: u64, old: u64) -> LinuxResult {
    const SIG_BLOCK: u64 = 0;
    const SIG_UNBLOCK: u64 = 1;
    const SIG_SETMASK: u64 = 2;
    let t = task::current_linux().ok_or(EINVAL)?;
    let new = if set != 0 {
        Some(read_u64(p, set)?)
    } else {
        None
    };
    let previous = {
        let mut s = t.signals.lock();
        let previous = s.mask;
        if let Some(new) = new {
            s.mask = match how {
                SIG_BLOCK => s.mask | new,
                SIG_UNBLOCK => s.mask & !new,
                SIG_SETMASK => new,
                _ => return Err(EINVAL),
            } & !signal::UNBLOCKABLE;
            t.update(&s);
        }
        previous
    };
    if old != 0 {
        write_user(p, old, &previous.to_le_bytes())?;
    }
    Ok(0)
}

/// `rt_sigpending(set, sigsetsize)`.
pub fn rt_sigpending(p: &Process, set: u64) -> LinuxResult {
    let t = task::current_linux().ok_or(EINVAL)?;
    let pending = t.signals.lock().pending;
    write_user(p, set, &pending.to_le_bytes())?;
    Ok(0)
}

/// Waits until a signal is to be taken; always `EINTR`.
fn wait_for_signal() -> LinuxResult {
    loop {
        if task::current_killed() {
            return Err(EINTR);
        }
        task::block_until(None);
    }
}

/// `pause()`.
pub fn pause() -> LinuxResult {
    wait_for_signal()
}

/// `rt_sigsuspend(mask, sigsetsize)`: waits with `mask` as the signal
/// mask; the old mask comes back when the handler returns.
pub fn rt_sigsuspend(p: &Process, mask: u64) -> LinuxResult {
    let t = task::current_linux().ok_or(EINVAL)?;
    let mask = read_u64(p, mask)?;
    {
        let mut s = t.signals.lock();
        s.saved_mask = Some(s.mask);
        s.mask = mask & !signal::UNBLOCKABLE;
        t.update(&s);
    }
    wait_for_signal()
}

/// `sigaltstack(ss, old_ss)` (P14 K9: handlers with `SA_ONSTACK` run on
/// it): the old one with `SS_ONSTACK` while on it; a new one is refused
/// (`EPERM`) while on the old one, `ENOMEM` below `MINSIGSTKSZ`.
pub fn sigaltstack(p: &Process, ss: u64, old: u64, rsp: u64) -> LinuxResult {
    let t = task::current_linux().ok_or(EINVAL)?;
    let current = t.signals.lock().altstack;
    let on = signal::on_alt_stack(rsp, current);
    if old != 0 {
        let (sp, size, flags) = current;
        let flags = if flags & signal::SS_DISABLE != 0 || size == 0 {
            signal::SS_DISABLE
        } else if on {
            signal::SS_ONSTACK
        } else {
            0
        };
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&sp.to_le_bytes());
        b[8..12].copy_from_slice(&flags.to_le_bytes());
        b[16..24].copy_from_slice(&size.to_le_bytes());
        write_user(p, old, &b)?;
    }
    if ss != 0 {
        let b = read_user(p, ss, 24)?;
        let word = |i: usize| {
            let mut w = [0u8; 8];
            w.copy_from_slice(&b[i..i + 8]);
            u64::from_le_bytes(w)
        };
        let flags = u32::from_le_bytes([b[8], b[9], b[10], b[11]]);
        if on {
            return Err(super::errno::EPERM);
        }
        if flags & !(signal::SS_DISABLE | (1 << 31)) != 0 {
            return Err(EINVAL);
        }
        if flags & signal::SS_DISABLE != 0 {
            t.signals.lock().altstack = (0, 0, signal::SS_DISABLE);
        } else {
            if word(16) < signal::MINSIGSTKSZ {
                return Err(super::errno::ENOMEM);
            }
            t.signals.lock().altstack = (word(0), word(16), 0);
        }
    }
    Ok(0)
}

/// The Linux processes in process group `pgid`.
fn group(pgid: u64) -> Vec<Arc<Process>> {
    task::process::list()
        .into_iter()
        .filter(|(_, _, _, code)| code.is_none())
        .filter_map(|(id, ..)| task::process::find(id))
        .filter(|p| {
            p.linux()
                .is_some_and(|l| l.pgid.load(Ordering::Relaxed) == pgid)
        })
        .collect()
}

/// `kill(pid, sig)`.
pub fn kill(p: &Process, pid: i64, signo: u64) -> LinuxResult {
    if signo != 0 && !signal::valid(signo) {
        return Err(EINVAL);
    }
    let targets: Vec<Arc<Process>> = if pid > 0 {
        let target = task::process::find(pid as u64).ok_or(ESRCH)?;
        if task::process::exit_status(pid as u64).is_some() {
            return Err(ESRCH);
        }
        alloc::vec![target]
    } else {
        let pgid = match pid {
            0 | -1 => p.linux().map_or(p.id(), |l| l.pgid.load(Ordering::Relaxed)),
            _ => pid.unsigned_abs(),
        };
        let g = group(pgid);
        if g.is_empty() {
            return Err(ESRCH);
        }
        g
    };
    if signo == 0 {
        return Ok(0);
    }
    for target in &targets {
        send_process(target, user_info(signo as u32, p.id()));
    }
    Ok(0)
}

/// `tgkill(tgid, tid, sig)` (`tgid` 0: `tkill`).
pub fn tgkill(p: &Process, tgid: u64, tid: u64, signo: u64) -> LinuxResult {
    if signo != 0 && !signal::valid(signo) {
        return Err(EINVAL);
    }
    let owner = if tgid == 0 || tgid == p.id() {
        None
    } else {
        Some(task::process::find(tgid).ok_or(ESRCH)?)
    };
    let linux = match &owner {
        Some(o) => o.linux(),
        None => p.linux(),
    }
    .ok_or(ESRCH)?;
    // tkill from another process: the main thread of process `tid`.
    let t = match linux.thread(tid) {
        Some(t) => t,
        None if tgid == 0 => {
            let other = task::process::find(tid).ok_or(ESRCH)?;
            other.linux().and_then(|l| l.thread(tid)).ok_or(ESRCH)?
        }
        None => return Err(ESRCH),
    };
    if signo == 0 {
        return Ok(0);
    }
    send_thread(
        &t,
        Info {
            signo: signo as u32,
            code: signal::SI_TKILL,
            pid: p.id() as u32,
            ..Info::default()
        },
    );
    Ok(0)
}

/// Processes with an armed `ITIMER_REAL`.
static TIMED: SpinLock<Vec<ProcessId>> = SpinLock::new(Vec::new());
/// The timer thread (0 until started).
static TIMER_THREAD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The timer thread: sleeps until the earliest `ITIMER_REAL` deadline,
/// sends `SIGALRM`, re-arms periodic timers.
fn timer_thread() {
    loop {
        let now = frame::time::now_ns();
        let mut next: Option<u64> = None;
        let pids: Vec<ProcessId> = TIMED.lock().clone();
        let mut gone = Vec::new();
        for pid in pids {
            let Some(process) = task::process::find(pid) else {
                gone.push(pid);
                continue;
            };
            let Some(linux) = process.linux() else {
                gone.push(pid);
                continue;
            };
            let fire = {
                let mut t = linux.itimer.lock();
                if t.deadline == 0 {
                    gone.push(pid);
                    false
                } else if t.deadline <= now {
                    t.deadline = if t.interval > 0 { now + t.interval } else { 0 };
                    if t.deadline != 0 {
                        next = Some(next.map_or(t.deadline, |n: u64| n.min(t.deadline)));
                    }
                    true
                } else {
                    next = Some(next.map_or(t.deadline, |n: u64| n.min(t.deadline)));
                    false
                }
            };
            if fire {
                send_process(
                    &process,
                    Info {
                        signo: sig::SIGALRM,
                        code: signal::SI_KERNEL,
                        ..Info::default()
                    },
                );
            }
        }
        if !gone.is_empty() {
            let mut timed = TIMED.lock();
            timed.retain(|pid| {
                !gone.contains(pid)
                    || task::process::find(*pid)
                        .and_then(|p| p.linux().map(|l| l.itimer.lock().deadline != 0))
                        .unwrap_or(false)
            });
        }
        task::block_until(next);
    }
}

/// Arms (or disarms) the calling process's `ITIMER_REAL`; returns the old
/// one (time left, interval).
fn set_itimer(p: &Process, new: Itimer) -> Result<Itimer, i64> {
    let linux = p.linux().ok_or(EINVAL)?;
    let now = frame::time::now_ns();
    let old = {
        let mut t = linux.itimer.lock();
        let old = *t;
        *t = new;
        old
    };
    if new.deadline != 0 {
        {
            let mut timed = TIMED.lock();
            if !timed.contains(&p.id()) {
                timed.push(p.id());
            }
        }
        let thread = TIMER_THREAD.load(Ordering::Acquire);
        if thread == 0 {
            match task::spawn("linux-timer", timer_thread) {
                Ok(id) => TIMER_THREAD.store(id, Ordering::Release),
                Err(_) => return Err(EAGAIN),
            }
        } else {
            task::wake(thread);
        }
    }
    Ok(Itimer {
        deadline: old.deadline.saturating_sub(now),
        interval: old.interval,
    })
}

/// `alarm(seconds)`: the seconds left of the previous alarm.
pub fn alarm(p: &Process, seconds: u64) -> LinuxResult {
    let new = Itimer {
        deadline: if seconds == 0 {
            0
        } else {
            frame::time::now_ns() + seconds.saturating_mul(1_000_000_000)
        },
        interval: 0,
    };
    let old = set_itimer(p, new)?;
    Ok(old.deadline.div_ceil(1_000_000_000))
}

/// A `struct timeval` (seconds, microseconds) in nanoseconds.
fn timeval(p: &Process, ptr: u64) -> Result<u64, i64> {
    let sec = read_u64(p, ptr)?;
    let usec = read_u64(p, ptr + 8)?;
    if usec >= 1_000_000 {
        return Err(EINVAL);
    }
    Ok(sec
        .saturating_mul(1_000_000_000)
        .saturating_add(usec * 1000))
}

fn put_itimerval(p: &Process, ptr: u64, t: Itimer) -> Result<(), i64> {
    let mut b = [0u8; 32];
    let tv = |ns: u64| ((ns / 1_000_000_000), (ns % 1_000_000_000) / 1000);
    let (s, u) = tv(t.interval);
    b[0..8].copy_from_slice(&s.to_le_bytes());
    b[8..16].copy_from_slice(&u.to_le_bytes());
    let (s, u) = tv(t.deadline);
    b[16..24].copy_from_slice(&s.to_le_bytes());
    b[24..32].copy_from_slice(&u.to_le_bytes());
    write_user(p, ptr, &b)
}

/// `setitimer(which, new, old)`: `ITIMER_REAL` only.
pub fn setitimer(p: &Process, which: u64, new: u64, old: u64) -> LinuxResult {
    if which != 0 {
        return Err(EINVAL);
    }
    let interval = if new != 0 { timeval(p, new)? } else { 0 };
    let value = if new != 0 { timeval(p, new + 16)? } else { 0 };
    let set = Itimer {
        deadline: if value == 0 {
            0
        } else {
            frame::time::now_ns() + value
        },
        interval,
    };
    let previous = set_itimer(p, set)?;
    if old != 0 {
        put_itimerval(p, old, previous)?;
    }
    Ok(0)
}

/// `getitimer(which, value)`.
pub fn getitimer(p: &Process, which: u64, value: u64) -> LinuxResult {
    if which != 0 {
        return Err(EINVAL);
    }
    let linux = p.linux().ok_or(EINVAL)?;
    let t = *linux.itimer.lock();
    let left = if t.deadline == 0 {
        0
    } else {
        t.deadline.saturating_sub(frame::time::now_ns()).max(1)
    };
    put_itimerval(
        p,
        value,
        Itimer {
            deadline: left,
            interval: t.interval,
        },
    )?;
    Ok(0)
}

/// Ctrl-C on a terminal: `SIGINT` to process `pid` (a STELA program is
/// killed as before).
pub fn interrupt(pid: ProcessId) {
    send_kernel(pid, sig::SIGINT);
}
