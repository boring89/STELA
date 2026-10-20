// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The scheduler: one run queue per processor (only the boot processor
//! runs for now), round-robin, plus the sleep list, the blocked threads and
//! the zombies waiting to be freed.
//!
//! ```text
//!            spawn / wake / sleep over / preempted / yield
//!                 │
//!                 ▼
//!   ┌────────► ready ──── schedule() picks the front ───► running
//!   │                                                      │ │ │
//!   │   sleeping ◄──────── sleep_ns ────────────────────────┘ │ │
//!   │   blocked  ◄──────── block ─────────────────────────────┘ │
//!   │   zombie   ◄──────── exit ────────────────────────────────┘
//!   │      └──► freed by the next thread to run (after the switch)
//!   └── idle: runs when nothing is ready, never queued
//! ```
//!
//! Every operation runs with interrupts disabled (the per-CPU lock does
//! that), so on one processor nothing can interleave with it; the context
//! switch itself happens after the lock is released, still with
//! interrupts off.

use super::process::Process;
use super::queue::{RunQueue, SleepList};
use super::{TIME_SLICE, ThreadId};
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use frame::sync::{IrqGuard, SpinLock};
use frame::task::{OutOfMemory, TaskContext};

/// Where a thread is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// In the run queue.
    Ready,
    /// On a processor.
    Running,
    /// In the sleep list until the given `now_ns`.
    Sleeping(u64),
    /// Waiting for [`wake`].
    Blocked,
    /// Finished; freed after the next switch.
    Zombie,
}

/// A kernel thread control block.
pub struct Thread {
    id: ThreadId,
    name: &'static str,
    state: State,
    /// Scheduling priority; stored, not used yet (single round-robin level).
    #[allow(dead_code)]
    priority: u8,
    /// Ticks left in the current time slice.
    slice: u32,
    /// A wake-up that arrived before the thread blocked.
    wake_pending: bool,
    context: Arc<TaskContext>,
    /// What a kernel thread runs, taken on its first start.
    start: Option<Box<dyn FnOnce() + Send>>,
    /// Where a user thread starts: (entry, stack pointer, argument).
    user_start: Option<(u64, u64, u64)>,
    /// The process of a user thread; `None` for kernel threads.
    process: Option<Arc<Process>>,
}

/// The per-processor scheduler state. Threads are boxed everywhere so that
/// moving one between the containers moves a pointer, not the whole
/// control block.
#[allow(clippy::vec_box)]
struct Scheduler {
    current: Option<Box<Thread>>,
    ready: RunQueue<Box<Thread>>,
    sleeping: SleepList<Box<Thread>>,
    blocked: Vec<Box<Thread>>,
    /// The idle thread while it is not running.
    idle: Option<Box<Thread>>,
    idle_id: ThreadId,
    zombies: Vec<Box<Thread>>,
}

/// Processors the scheduler has room for.
const MAX_CPUS: usize = 16;

static CPUS: [SpinLock<Option<Scheduler>>; MAX_CPUS] = [const { SpinLock::new(None) }; MAX_CPUS];

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn new_id() -> ThreadId {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn this_cpu() -> &'static SpinLock<Option<Scheduler>> {
    &CPUS[frame::cpu::current().id() as usize]
}

/// Runs `f` on this processor's scheduler.
fn with<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    let mut guard = this_cpu().lock();
    match guard.as_mut() {
        Some(s) => f(s),
        None => panic!("task: scheduler used before task::init()"),
    }
}

fn current_mut(s: &mut Scheduler) -> &mut Thread {
    match s.current.as_mut() {
        Some(t) => t,
        None => panic!("task: no current thread"),
    }
}

/// Turns the code running now into the thread "main" and creates this
/// processor's idle thread. Call once, after `frame::init`.
pub fn init() {
    let main = Box::new(Thread {
        id: new_id(),
        name: "main",
        state: State::Running,
        priority: 0,
        slice: TIME_SLICE,
        wake_pending: false,
        context: frame::task::current(),
        start: None,
        user_start: None,
        process: None,
    });
    let idle_id = new_id();
    let idle = match TaskContext::new(idle_entry, 0, false) {
        Ok(context) => Box::new(Thread {
            id: idle_id,
            name: "idle",
            state: State::Ready,
            priority: u8::MAX,
            slice: TIME_SLICE,
            wake_pending: false,
            context,
            start: None,
            user_start: None,
            process: None,
        }),
        Err(OutOfMemory) => panic!("task: no memory for the idle thread"),
    };
    let mut cpu = this_cpu().lock();
    assert!(cpu.is_none(), "task: init() twice");
    *cpu = Some(Scheduler {
        current: Some(main),
        ready: RunQueue::new(),
        sleeping: SleepList::new(),
        blocked: Vec::new(),
        idle: Some(idle),
        idle_id,
        zombies: Vec::new(),
    });
    drop(cpu);
    frame::time::set_tick_hook(tick);
    frame::task::set_scheduler(preempt);
    frame::task::set_user_fault_handler(user_fault);
}

/// Runs on every timer tick (1 ms), inside the interrupt: wakes the
/// sleepers whose time has come and counts down the running thread's time
/// slice. When the slice is used up and someone else is ready, or the idle
/// thread runs while someone is ready, it asks for a reschedule at the end
/// of the interrupt.
fn tick() {
    let now = frame::time::now_ns();
    let resched = with(|s| {
        while let Some(mut t) = s.sleeping.pop_expired(now) {
            t.state = State::Ready;
            s.ready.push(t);
        }
        let idle_id = s.idle_id;
        let ready = !s.ready.is_empty();
        let t = current_mut(s);
        if t.id == idle_id {
            return ready;
        }
        t.slice = t.slice.saturating_sub(1);
        if t.slice == 0 {
            t.slice = TIME_SLICE;
            return ready;
        }
        false
    });
    if resched {
        frame::cpu::current().set_need_resched();
    }
}

/// The preemption point registered with `frame::task`: called at the end
/// of an interrupt that asked for a reschedule.
fn preempt() {
    schedule();
}

/// Sleeps for at least `ns` nanoseconds. Wake-ups happen on timer ticks,
/// so the sleep is rounded up to the next millisecond or so.
pub fn sleep_ns(ns: u64) {
    let _irq = IrqGuard::new();
    // Without an invariant TSC the clock counts whole ticks and may lag the
    // real time by up to one tick; one more makes the minimum hold.
    let lag = if frame::time::tsc_invariant() {
        0
    } else {
        1_000_000_000 / frame::time::TICK_HZ
    };
    let wake_at = frame::time::now_ns().saturating_add(ns).saturating_add(lag);
    with(|s| current_mut(s).state = State::Sleeping(wake_at));
    schedule();
}

/// Sleeps for at least `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    sleep_ns(ms.saturating_mul(1_000_000));
}

/// The idle thread: waits for interrupts, and gives the processor away as
/// soon as anything is ready.
fn idle_entry(_: usize) -> ! {
    reap();
    loop {
        if with(|s| s.ready.is_empty()) {
            frame::halt_until_interrupt();
        } else {
            yield_now();
        }
    }
}

/// Where every thread starts: frees what the previous thread left behind,
/// then runs the kernel thread's function (and exits), or enters user mode.
fn thread_entry(_: usize) -> ! {
    reap();
    let (start, user) = with(|s| {
        let t = current_mut(s);
        (t.start.take(), t.user_start.take())
    });
    if let Some((entry, rsp, arg)) = user {
        // The scheduler activated the process's address space.
        frame::task::enter_user(entry, rsp, arg);
    }
    if let Some(start) = start {
        start();
    }
    exit()
}

/// Creates a kernel thread running `f` and queues it. Returns its id.
pub fn spawn(
    name: &'static str,
    f: impl FnOnce() + Send + 'static,
) -> Result<ThreadId, OutOfMemory> {
    let context = TaskContext::new(thread_entry, 0, false)?;
    let id = new_id();
    let thread = Box::new(Thread {
        id,
        name,
        state: State::Ready,
        priority: 0,
        slice: TIME_SLICE,
        wake_pending: false,
        context,
        start: Some(Box::new(f)),
        user_start: None,
        process: None,
    });
    with(|s| s.ready.push(thread));
    Ok(id)
}

/// Why a user thread could not be created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// No memory for the kernel stack.
    OutOfMemory,
    /// The process is exiting.
    Exiting,
}

/// Creates a thread of `process` that starts in user mode at `entry`, with
/// stack pointer `rsp` and `arg` in rdi, and queues it.
pub fn spawn_user(
    process: &Arc<Process>,
    entry: u64,
    rsp: u64,
    arg: u64,
) -> Result<ThreadId, SpawnError> {
    let context =
        TaskContext::new(thread_entry, 0, true).map_err(|OutOfMemory| SpawnError::OutOfMemory)?;
    let id = new_id();
    let thread = Box::new(Thread {
        id,
        name: "user",
        state: State::Ready,
        priority: 0,
        slice: TIME_SLICE,
        wake_pending: false,
        context,
        start: None,
        user_start: Some((entry, rsp, arg)),
        process: Some(Arc::clone(process)),
    });
    let _irq = IrqGuard::new();
    if process.exiting() {
        return Err(SpawnError::Exiting);
    }
    process.thread_added();
    with(|s| s.ready.push(thread));
    Ok(id)
}

/// The process of the running thread (`None` for a kernel thread).
pub fn current_process() -> Option<Arc<Process>> {
    with(|s| current_mut(s).process.clone())
}

/// Ends every thread of the running thread's process with exit code
/// `code` (unless one was already decided) and switches away. The process
/// ends when the last of them has been freed.
pub fn exit_process(code: i64) -> ! {
    {
        let _irq = IrqGuard::new();
        let Some(process) = current_process() else {
            panic!("task: exit_process from a kernel thread");
        };
        kill(&process, code);
        // schedule() never returns here: nothing on this stack is dropped
        // after it, so the reference must go first.
        drop(process);
        schedule();
    }
    unreachable!("task: a zombie was scheduled");
}

/// Ends every thread of `process` with exit code `code` (unless one was
/// already decided). Threads that are not running become zombies at once;
/// if the running thread belongs to it, it becomes a zombie too and the
/// caller must call [`schedule`] (it does not return to user mode).
pub fn kill(process: &Arc<Process>, code: i64) {
    process.set_exit_code(code);
    let pid = process.id();
    // The containers hold boxes, so the predicate takes `&Box<Thread>`.
    #[allow(clippy::borrowed_box)]
    let ours = |t: &Box<Thread>| belongs_to(t, pid);
    with(|s| {
        let mut doomed = s.ready.take_where(ours);
        doomed.append(&mut s.sleeping.take_where(ours));
        let (mut blocked, others): (Vec<_>, Vec<_>) =
            core::mem::take(&mut s.blocked).into_iter().partition(ours);
        s.blocked = others;
        doomed.append(&mut blocked);
        for mut t in doomed {
            t.state = State::Zombie;
            s.zombies.push(t);
        }
        if let Some(t) = s.current.as_mut().filter(|t| ours(t)) {
            t.state = State::Zombie;
        }
    });
}

/// Whether `thread` is one of process `pid`'s.
fn belongs_to(thread: &Thread, pid: super::ProcessId) -> bool {
    thread.process.as_ref().is_some_and(|p| p.id() == pid)
}

/// The handler for exceptions in ring 3: reports them and ends the
/// process with exit code 128 + vector.
fn user_fault(frame: &frame::trap::TrapFrame, cr2: u64) -> ! {
    let process = current_process();
    let (pid, name) = process.as_ref().map_or((0, "?"), |p| (p.id(), p.name()));
    // CR2 only means something for a page fault.
    let mut fault_address = alloc::string::String::new();
    if frame.vector == 14 {
        let _ = core::fmt::Write::write_fmt(&mut fault_address, format_args!(" CR2={cr2:#x}"));
    }
    frame::kprintln!(
        "process {pid} ({name}): exception {} ({}), error code {:#x} at RIP={:#x} RSP={:#x}{fault_address}; killed",
        frame.vector,
        frame::trap::exception_name(frame.vector),
        frame.error_code,
        frame.rip,
        frame.rsp,
    );
    drop(process);
    exit_process(128 + frame.vector as i64)
}

/// The running thread's id.
pub fn current_id() -> ThreadId {
    with(|s| current_mut(s).id)
}

/// The running thread's name.
pub fn current_name() -> &'static str {
    with(|s| current_mut(s).name)
}

/// Gives the processor to the next ready thread, if any; the caller goes
/// to the back of the run queue.
pub fn yield_now() {
    schedule();
}

/// Ends the running thread. Its stack and control block are freed by the
/// next thread to run.
pub fn exit() -> ! {
    {
        let _irq = IrqGuard::new();
        with(|s| current_mut(s).state = State::Zombie);
        schedule();
    }
    unreachable!("task: a zombie was scheduled");
}

/// Puts the caller to sleep until [`wake`] is called for it. A wake-up
/// that came while it was still running makes this return at once.
pub fn block() {
    let _irq = IrqGuard::new();
    let pending = with(|s| {
        let t = current_mut(s);
        if t.wake_pending {
            t.wake_pending = false;
            true
        } else {
            t.state = State::Blocked;
            false
        }
    });
    if !pending {
        schedule();
    }
}

/// Makes thread `id` ready if it is blocked; if it is running or ready,
/// its next [`block`] returns at once. Unknown ids are ignored.
pub fn wake(id: ThreadId) {
    with(|s| {
        if let Some(t) = s.current.as_mut().filter(|t| t.id == id) {
            // Blocked but not switched away yet (between `block` deciding
            // and `schedule` running, with interrupts off): just run on.
            if t.state == State::Blocked {
                t.state = State::Running;
            } else {
                t.wake_pending = true;
            }
            return;
        }
        if let Some(i) = s.blocked.iter().position(|t| t.id == id) {
            let mut t = s.blocked.swap_remove(i);
            t.state = State::Ready;
            s.ready.push(t);
            return;
        }
        // Ready: remember the wake-up for its next `block`.
        if let Some(t) = s.ready.iter_mut().find(|t| t.id == id) {
            t.wake_pending = true;
        }
    });
}

/// Picks the next thread and switches to it. The running thread goes where
/// its state says: back to the run queue (running), the sleep list, the
/// blocked list, or the zombies.
pub fn schedule() {
    let _irq = IrqGuard::new();
    let next = with(|s| {
        let Some(mut prev) = s.current.take() else {
            panic!("task: schedule() without a current thread");
        };
        match prev.state {
            State::Running | State::Ready if prev.id == s.idle_id => {
                prev.state = State::Ready;
                s.idle = Some(prev);
            }
            State::Running | State::Ready => {
                prev.state = State::Ready;
                s.ready.push(prev);
            }
            State::Sleeping(at) => s.sleeping.insert(at, prev),
            State::Blocked => s.blocked.push(prev),
            State::Zombie => s.zombies.push(prev),
        }
        let mut next = match s.ready.pop().or_else(|| s.idle.take()) {
            Some(t) => t,
            None => panic!("task: nothing to run, not even idle"),
        };
        next.state = State::Running;
        next.slice = TIME_SLICE;
        // A user thread needs its process's address space; kernel threads
        // run in whichever is active (the kernel half is the same).
        if let Some(process) = &next.process {
            process.space().activate();
        }
        let context = Arc::clone(&next.context);
        s.current = Some(next);
        context
    });
    frame::task::switch_to(next);
    reap();
}

/// Frees the threads that have exited. Their stacks are no longer in use:
/// the switch away from each of them is complete.
fn reap() {
    let zombies = with(|s| core::mem::take(&mut s.zombies));
    for mut zombie in zombies {
        let process = zombie.process.take();
        drop(zombie);
        if let Some(process) = process {
            // May end the process; its address space goes with the last
            // reference, which may be this one.
            process.thread_reaped();
        }
    }
}

/// Scheduler counts, for tests and diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Threads in the run queue.
    pub ready: usize,
    /// Threads in the sleep list.
    pub sleeping: usize,
    /// Blocked threads.
    pub blocked: usize,
    /// Exited threads not freed yet.
    pub zombies: usize,
}

/// The current counts of this processor's scheduler.
pub fn stats() -> Stats {
    with(|s| Stats {
        ready: s.ready.len(),
        sleeping: s.sleeping.len(),
        blocked: s.blocked.len(),
        zombies: s.zombies.len(),
    })
}
