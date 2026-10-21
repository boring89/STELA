// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mechanism of threads: execution contexts, kernel stacks and the
//! context switch. The policy (which thread runs next, when to sleep) is
//! the kernel's; this module only knows how to save one context and
//! continue another.
//!
//! A [`TaskContext`] is shared through an [`Arc`]. The per-CPU area holds
//! the running one, so its stack cannot be freed while the processor runs
//! on it, whatever the kernel does with its own references:
//!
//! ```text
//!  switch_to(next):   per-CPU current = next
//!                     per-CPU previous = the old current   (still on its stack)
//!                     ── switch stacks ──
//!  on the new stack:  drop per-CPU previous                (safe now)
//! ```
//!
//! So a thread that has exited is freed by the next one to run: when the
//! kernel has dropped its references, the last one goes in that drop.

pub mod stack;

use crate::arch::x86_64::{context, gdt, msr};
use crate::cpu;
use crate::sync::Once;
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
pub use stack::{KERNEL_STACK_SIZE, KernelStack};

/// What a thread starts with: an entry function and its argument. The
/// entry runs with interrupts enabled and never returns.
pub type Entry = fn(usize) -> !;

/// The saved state of a thread that is not running, its kernel stack, and
/// (for threads that run user code) its FPU/SSE state.
pub struct TaskContext {
    /// Saved stack pointer while suspended.
    rsp: UnsafeCell<u64>,
    /// `None` for the boot thread, which runs on the loader's stack.
    stack: Option<KernelStack>,
    /// FXSAVE area, switched eagerly; `None` for kernel-only threads (the
    /// kernel is compiled without SSE, so it never touches this state).
    fpu: Option<Box<UnsafeCell<context::FxArea>>>,
    /// IA32_FS_BASE for user threads (Linux programs keep their thread
    /// pointer there, `arch_prctl(ARCH_SET_FS)`); loaded when the thread
    /// is switched to. User code cannot change it itself (CR4.FSGSBASE is
    /// clear, so `wrfsbase` faults).
    fs_base: AtomicU64,
}

// SAFETY: `rsp` and `fpu` are only accessed by `switch_to`, on the
// processor that runs or suspends the context, with interrupts disabled;
// a context runs on one processor at a time.
unsafe impl Sync for TaskContext {}
// SAFETY: as above; the stack is plain owned memory.
unsafe impl Send for TaskContext {}

/// Why a context could not be created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfMemory;

impl TaskContext {
    /// A new context with its own kernel stack. The first switch to it
    /// calls `entry(arg)`. With `fpu`, it gets an FPU/SSE save area,
    /// initialised to the FNINIT state.
    pub fn new(entry: Entry, arg: usize, fpu: bool) -> Result<Arc<TaskContext>, OutOfMemory> {
        let stack = KernelStack::new().ok_or(OutOfMemory)?;
        let top = stack.top();
        // The initial frame, from the saved rsp upwards: r15 r14 r13 r12
        // rbx rbp, return address, padding (see arch/x86_64/context.rs).
        let frame: [u64; 8] = [
            0,
            0,
            arg as u64,
            entry as usize as u64,
            0,
            0,
            context::trampoline(),
            0,
        ];
        let rsp = top - 8 * frame.len() as u64;
        // SAFETY: the stack's pages are mapped writable and owned by
        // `stack`, which nothing else references yet; `rsp..top` lies at
        // its top end and is 8-byte aligned.
        unsafe { core::ptr::write(rsp as *mut [u64; 8], frame) };
        Ok(Arc::new(TaskContext {
            rsp: UnsafeCell::new(rsp),
            stack: Some(stack),
            fpu: fpu.then(|| Box::new(UnsafeCell::new(context::FxArea::initial()))),
            fs_base: AtomicU64::new(0),
        }))
    }

    /// Top of the kernel stack, if the context has its own.
    pub fn stack_top(&self) -> Option<u64> {
        self.stack.as_ref().map(KernelStack::top)
    }

    /// Whether the context saves the FPU/SSE state.
    pub fn has_fpu(&self) -> bool {
        self.fpu.is_some()
    }

    /// The FS base the thread runs user code with.
    pub fn fs_base(&self) -> u64 {
        self.fs_base.load(Ordering::Relaxed)
    }
}

/// Why [`set_fs_base`] refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadFsBase;

/// Sets the running user thread's FS base (`arch_prctl(ARCH_SET_FS)`):
/// stored in its context and loaded into IA32_FS_BASE now. The address
/// must be in the user half (a non-canonical value would make `wrmsr`
/// fault); the running thread must be a user thread.
pub fn set_fs_base(base: u64) -> Result<(), BadFsBase> {
    if base >= crate::mm::USER_END {
        return Err(BadFsBase);
    }
    let current = current();
    if !current.has_fpu() {
        return Err(BadFsBase);
    }
    let _irq = crate::sync::IrqGuard::new();
    current.fs_base.store(base, Ordering::Relaxed);
    // SAFETY: IA32_FS_BASE exists on every x86-64 processor (Vol. 4 Table
    // 2-2); the kernel never uses FS, so moving it only affects this
    // thread's user code. The value is canonical (checked above).
    unsafe { msr::write(msr::IA32_FS_BASE, base) };
    Ok(())
}

/// The kernel's scheduler entry for preemption, see [`set_scheduler`].
static SCHEDULER: Once<fn()> = Once::new();

/// Registers the function called at the end of an interrupt when the
/// handler asked for a reschedule (`PerCpu::set_need_resched`). It runs
/// with interrupts disabled, on the interrupted thread's stack, and may
/// switch to another thread. Only the first registration counts.
pub fn set_scheduler(schedule: fn()) {
    let _ = SCHEDULER.set(schedule);
}

/// Called by `irq::dispatch` after the EOI when a reschedule was asked
/// for.
pub(crate) fn preempt() {
    if let Some(schedule) = SCHEDULER.get() {
        schedule();
    }
}

/// What the kernel does with an exception in ring 3, see
/// [`set_user_fault_handler`].
pub type UserFaultHandler = fn(&mut crate::trap::TrapFrame, u64);

static USER_FAULT: Once<UserFaultHandler> = Once::new();

/// Registers the function called for an exception raised in ring 3, with
/// the saved registers and CR2. It runs with interrupts disabled on the
/// thread's kernel stack. It either ends the thread (never returns) or
/// changes the registers (a Linux signal handler, `docs/linux.md`) and
/// returns; the thread then goes back to ring 3 through the user-return
/// hook. Only the first registration counts.
pub fn set_user_fault_handler(handler: UserFaultHandler) {
    let _ = USER_FAULT.set(handler);
}

/// Called by the exception dispatcher for a ring-3 exception.
pub(crate) fn user_fault(frame: &mut crate::trap::TrapFrame, cr2: u64) {
    match USER_FAULT.get() {
        Some(handler) => handler(frame, cr2),
        None => panic!("{}", crate::trap::Report { frame, cr2 }),
    }
    user_return(&mut crate::syscall::UserFrame::Trap(frame));
}

/// The kernel's check before returning to ring 3, see
/// [`set_user_return_hook`].
pub type UserReturnHook = fn(&mut crate::syscall::UserFrame<'_>);

static USER_RETURN: Once<UserReturnHook> = Once::new();

/// Registers a function called on every return to ring 3 from a system
/// call, an interrupt or an exception, with interrupts disabled, after any
/// reschedule. Nothing of the kernel is on the thread's stack below it
/// then, so it may end the thread (never return); it may also replace the
/// user registers (signal delivery). Only the first registration counts.
pub fn set_user_return_hook(hook: UserReturnHook) {
    let _ = USER_RETURN.set(hook);
}

/// Called by `irq::dispatch`, `syscall::dispatch` and [`user_fault`]
/// before going back to ring 3.
pub(crate) fn user_return(frame: &mut crate::syscall::UserFrame<'_>) {
    if let Some(hook) = USER_RETURN.get() {
        hook(frame);
    }
}

/// Lowest address the user stack pointer and entry point may have
/// (inclusive) and the end of the user half (exclusive).
const USER_RANGE: core::ops::Range<u64> = 0..crate::mm::USER_END;

/// Leaves the kernel for ring 3: `entry` runs with `rsp` as stack pointer
/// and `arg` in rdi, interrupts enabled, in the active address space. The
/// running thread must have its own kernel stack and an FPU area (made
/// with `TaskContext::new(.., fpu: true)`); everything on that stack is
/// abandoned, the next interrupt from ring 3 starts at its top.
pub fn enter_user(entry: u64, rsp: u64, arg: u64) -> ! {
    assert!(
        USER_RANGE.contains(&entry) && USER_RANGE.contains(&rsp),
        "task: enter_user outside the user half ({entry:#x}, {rsp:#x})"
    );
    let current = current();
    let Some(top) = current.stack_top() else {
        panic!("task: enter_user on a thread without its own kernel stack");
    };
    assert!(
        current.has_fpu(),
        "task: enter_user on a thread without FPU state"
    );
    drop(current);
    let _irq = crate::sync::IrqGuard::new();
    gdt::set_kernel_stack(top);
    cpu::current().set_kernel_rsp(top);
    // SAFETY: RSP0 and kernel_rsp are this thread's own stack top (set just
    // above, interrupts off); nothing below it is needed since this never
    // returns. The current context stays alive in the per-CPU area.
    unsafe { context::enter_user(entry, rsp, arg) }
}

/// Like [`enter_user`], with every register given (a Linux thread made by
/// `clone`, a `fork` child): the registers are sanitized first
/// ([`UserRegs::sanitized`](crate::trap::UserRegs::sanitized)); invalid
/// ones panic, so the caller checks them before.
pub fn enter_user_regs(regs: &crate::trap::UserRegs) -> ! {
    let Ok(regs) = regs.sanitized() else {
        panic!("task: enter_user_regs outside the user half ({regs:x?})");
    };
    let current = current();
    let Some(top) = current.stack_top() else {
        panic!("task: enter_user_regs on a thread without its own kernel stack");
    };
    assert!(
        current.has_fpu(),
        "task: enter_user_regs on a thread without FPU state"
    );
    drop(current);
    let _irq = crate::sync::IrqGuard::new();
    gdt::set_kernel_stack(top);
    cpu::current().set_kernel_rsp(top);
    // SAFETY: as in `enter_user`; `regs` lives on this stack above the
    // frame `iretq` uses, and RIP/RSP are in the user half (sanitized).
    unsafe { context::enter_user_regs(&regs) }
}

/// The running user thread's x87/SSE state (FXSAVE format, Intel SDM
/// Vol. 1 Table 10-2). The kernel never uses these registers, so while a
/// user thread runs kernel code they still hold its state.
pub fn fpu_state() -> [u8; 512] {
    let mut area = context::FxArea::initial();
    let _irq = crate::sync::IrqGuard::new();
    // SAFETY: `area` is a local, aligned FXSAVE area.
    unsafe { context::fxsave(&mut area) };
    area.0
}

/// Loads `state` (FXSAVE format) into the running user thread's x87/SSE
/// registers, e.g. from a signal frame. MXCSR's reserved bits are cleared
/// first (FXRSTOR would raise #GP, Vol. 1 10.5.1.1).
pub fn set_fpu_state(state: &[u8; 512]) {
    let mut area = context::FxArea(*state);
    let mask = context::mxcsr_mask();
    let mxcsr = u32::from_le_bytes([area.0[24], area.0[25], area.0[26], area.0[27]]) & mask;
    area.0[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    let _irq = crate::sync::IrqGuard::new();
    // SAFETY: a full 512-byte aligned area with valid MXCSR; the other
    // fields have no reserved bits FXRSTOR checks.
    unsafe { context::fxrstor(&area) };
}

/// Resets the running user thread's x87/SSE state to the FNINIT state
/// (`execve`).
pub fn reset_fpu() {
    let area = context::FxArea::initial();
    let _irq = crate::sync::IrqGuard::new();
    // SAFETY: the initial area is a valid FXSAVE image.
    unsafe { context::fxrstor(&area) };
}

impl TaskContext {
    /// Gives this (not yet running) context the running thread's FPU
    /// state, as `clone` and `fork` do.
    pub fn copy_fpu_from_current(&self) {
        let Some(fpu) = &self.fpu else { return };
        let _irq = crate::sync::IrqGuard::new();
        // SAFETY: the context has not run yet, so nothing else uses its
        // area; FXSAVE stores the live (current thread's) state.
        unsafe { context::fxsave(fpu.get()) };
    }

    /// Sets the FS base this (not yet running) context starts with.
    pub fn set_initial_fs_base(&self, base: u64) -> Result<(), BadFsBase> {
        if base >= crate::mm::USER_END {
            return Err(BadFsBase);
        }
        self.fs_base.store(base, Ordering::Relaxed);
        Ok(())
    }
}

/// Makes the code running now (the boot thread on the loader's stack) the
/// processor's current context. Called by `frame::init`.
pub(crate) fn init() {
    context::enable_sse();
    let boot = Arc::new(TaskContext {
        rsp: UnsafeCell::new(0),
        stack: None,
        fpu: None,
        fs_base: AtomicU64::new(0),
    });
    let _irq = crate::sync::IrqGuard::new();
    // SAFETY: interrupts are disabled and this processor is the only user
    // of its task cells.
    unsafe { *cpu::current().current_task.get() = Some(boot) };
}

/// The running context.
pub fn current() -> Arc<TaskContext> {
    let _irq = crate::sync::IrqGuard::new();
    // SAFETY: interrupts are disabled, so no switch can change the cell
    // while it is read; only this processor uses it.
    let current = unsafe { &*cpu::current().current_task.get() };
    match current {
        Some(context) => Arc::clone(context),
        None => panic!("task: current() before frame::init()"),
    }
}

/// Suspends the running context and continues `next`; returns when
/// something switches back. Interrupts must be disabled (panics
/// otherwise): a switch interrupted half-way would corrupt both contexts.
/// Switching to the running context does nothing.
pub fn switch_to(next: Arc<TaskContext>) {
    assert!(
        !crate::sync::interrupts_enabled(),
        "task: switch_to with interrupts enabled"
    );
    let cpu = cpu::current();
    // SAFETY: interrupts are disabled and only this processor uses its
    // task cells; the references end before the stacks are switched.
    // IA32_FS_BASE exists on every x86-64 processor and the kernel does not
    // use FS; the stored value was checked to be canonical.
    let (save_rsp, load_rsp) = unsafe {
        let current = &mut *cpu.current_task.get();
        let Some(prev) = current.take() else {
            panic!("task: switch_to before frame::init()");
        };
        if Arc::ptr_eq(&prev, &next) {
            *current = Some(prev);
            return;
        }
        if let Some(fpu) = &prev.fpu {
            context::fxsave(fpu.get());
        }
        if let Some(fpu) = &next.fpu {
            context::fxrstor(fpu.get());
            // User threads get their own FS base (0 unless they set one),
            // so no program sees another one's.
            msr::write(msr::IA32_FS_BASE, next.fs_base.load(Ordering::Relaxed));
        }
        if let Some(top) = next.stack_top() {
            gdt::set_kernel_stack(top);
            cpu.set_kernel_rsp(top);
        }
        let save_rsp = prev.rsp.get();
        let load_rsp = *next.rsp.get();
        *current = Some(next);
        // Keeps the old context (and so its stack) alive until we are off
        // it; `finish_switch` drops it on the new stack.
        let previous = &mut *cpu.previous_task.get();
        *previous = Some(prev);
        (save_rsp, load_rsp)
    };
    // SAFETY: `save_rsp` points into the old context, kept alive by
    // `previous_task`; `load_rsp` was saved by an earlier switch away from
    // `next` (or built by `TaskContext::new`), and `next`'s stack stays
    // mapped while `current_task` holds it. Interrupts are disabled.
    unsafe { context::switch(save_rsp, load_rsp) };
    finish_switch();
}

/// Drops the context that switched to this one (on this side of the
/// switch, its stack is no longer in use).
fn finish_switch() {
    // SAFETY: interrupts are disabled (both callers run right after a
    // switch, which requires it); only this processor uses the cell. The
    // value is moved out before it is dropped.
    let previous = unsafe { (*cpu::current().previous_task.get()).take() };
    drop(previous);
}

/// Where a new context starts, called by the trampoline with the entry
/// and argument from `TaskContext::new`.
pub(crate) extern "C" fn thread_start(entry: usize, arg: usize) -> ! {
    finish_switch();
    crate::irq::enable();
    // SAFETY: `entry` is the `Entry` that `TaskContext::new` stored in r12;
    // function pointers and usize have the same size.
    let entry: Entry = unsafe { core::mem::transmute::<usize, Entry>(entry) };
    entry(arg)
}
