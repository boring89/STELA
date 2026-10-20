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
    /// What the thread runs, taken on its first start.
    start: Option<Box<dyn FnOnce() + Send>>,
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

/// Where every kernel thread starts: frees what the previous thread left
/// behind, runs the thread's function, and exits.
fn thread_entry(_: usize) -> ! {
    reap();
    let start = with(|s| current_mut(s).start.take());
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
    });
    with(|s| s.ready.push(thread));
    Ok(id)
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
    drop(zombies);
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
