// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The STELA user-space runtime.
//!
//! - [`entry!`] defines the program's `_start`: it calls `main` and exits
//!   the process with the code it returns.
//! - Safe wrappers for every system call ([`exit`], [`thread_exit`],
//!   [`spawn`], [`yield_now`], [`sleep_ns`], [`now_ns`]).
//! - Handles, ports, messages and shared memory ([`ipc`]).
//! - [`console_write`], [`debug_print!`] / [`debug_println!`]: output
//!   through the console service (bootstrap handle `[1]`).
//! - The panic handler: prints the message and exits with code 101.
//!
//! The `syscall` instruction in [`sys`], [`ThreadStack`] and the copies in
//! [`ipc::Mapping`] are the only `unsafe` code a STELA program needs.

#![no_std]

pub use abi;
pub use abi::{Error, Handle, MESSAGE_DATA, Message, PORT_QUEUE_DEPTH, Rights, bootstrap, tag};

pub mod fs;
pub mod ipc;
pub mod sys;

use core::cell::UnsafeCell;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
use sys::{syscall0, syscall1, syscall3};

/// Defines the program entry point `_start`, which runs `$main` (a
/// `fn() -> i64`) and exits the process with its result.
///
/// `_start` needs `#[unsafe(no_mangle)]`; generated here, it does not count
/// against the program's own `#![forbid(unsafe_code)]`.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        extern "C" fn _start() -> ! {
            let main: fn() -> i64 = $main;
            $crate::exit(main())
        }
    };
}

/// Ends the process (every thread) with exit code `code`.
pub fn exit(code: i64) -> ! {
    let _ = syscall1(abi::Syscall::ProcessExit, code as u64);
    // The kernel never returns from ProcessExit.
    loop {
        core::hint::spin_loop();
    }
}

/// Ends the calling thread. The process ends with its last thread.
pub fn thread_exit() -> ! {
    let _ = syscall0(abi::Syscall::ThreadExit);
    loop {
        core::hint::spin_loop();
    }
}

/// Lets other threads run.
pub fn yield_now() {
    let _ = syscall0(abi::Syscall::Yield);
}

/// Sleeps at least `ns` nanoseconds.
pub fn sleep_ns(ns: u64) {
    let _ = syscall1(abi::Syscall::SleepNs, ns);
}

/// Sleeps at least `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    sleep_ns(ms.saturating_mul(1_000_000));
}

/// Nanoseconds since boot.
pub fn now_ns() -> u64 {
    syscall0(abi::Syscall::ClockNowNs).unwrap_or(0)
}

/// Writes `bytes` to the console: `CONSOLE_WRITE` messages of up to 112
/// bytes to the console service (bootstrap handle `[1]`). When its queue
/// is full, waits for room. Returns how many bytes were sent.
pub fn console_write(bytes: &[u8]) -> Result<usize, Error> {
    let mut written = 0;
    for chunk in bytes.chunks(MESSAGE_DATA) {
        let msg = Message::with_data(tag::CONSOLE_WRITE, chunk);
        loop {
            match ipc::port_send(bootstrap::CONSOLE, &msg) {
                Ok(()) => break,
                Err(Error::Full) => yield_now(),
                Err(e) => return Err(e),
            }
        }
        written += chunk.len();
    }
    Ok(written)
}

/// Collects formatted text and sends it in as few messages as possible,
/// so that a short line is not split between threads.
struct ConsoleBuffer {
    bytes: [u8; 256],
    len: usize,
}

impl ConsoleBuffer {
    fn flush(&mut self) {
        let _ = console_write(&self.bytes[..self.len]);
        self.len = 0;
    }
}

impl fmt::Write for ConsoleBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for chunk in s.as_bytes().chunks(self.bytes.len()) {
            if self.len + chunk.len() > self.bytes.len() {
                self.flush();
            }
            self.bytes[self.len..self.len + chunk.len()].copy_from_slice(chunk);
            self.len += chunk.len();
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let mut buffer = ConsoleBuffer {
        bytes: [0; 256],
        len: 0,
    };
    let _ = fmt::Write::write_fmt(&mut buffer, args);
    buffer.flush();
}

/// Prints to the console (through the console service).
#[macro_export]
macro_rules! debug_print {
    ($($arg:tt)*) => {
        $crate::_print(format_args!($($arg)*))
    };
}

/// Prints to the console, followed by a newline.
#[macro_export]
macro_rules! debug_println {
    () => {
        $crate::debug_print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}

/// What a new thread runs, kept at the start of its [`ThreadStack`].
#[repr(C)]
struct Start {
    entry: fn(u64),
    arg: u64,
}

/// Memory for one extra thread's stack (`N` bytes), usable once. Put it in
/// a `static`; [`spawn`] takes it.
#[repr(C, align(16))]
pub struct ThreadStack<const N: usize> {
    /// Must stay the first field: the new thread finds it at the address
    /// it is given.
    start: UnsafeCell<Start>,
    taken: AtomicBool,
    memory: UnsafeCell<[u8; N]>,
}

// SAFETY: `start` is written once, by the `spawn` call that wins `taken`,
// before the thread that reads it exists; `memory` is only used as that
// thread's stack, through the stack pointer, never through references.
unsafe impl<const N: usize> Sync for ThreadStack<N> {}

fn no_entry(_: u64) {}

impl<const N: usize> ThreadStack<N> {
    /// An unused stack.
    pub const fn new() -> ThreadStack<N> {
        ThreadStack {
            start: UnsafeCell::new(Start {
                entry: no_entry,
                arg: 0,
            }),
            taken: AtomicBool::new(false),
            memory: UnsafeCell::new([0; N]),
        }
    }
}

impl<const N: usize> Default for ThreadStack<N> {
    fn default() -> ThreadStack<N> {
        ThreadStack::new()
    }
}

/// Where every thread made by [`spawn`] begins: runs the entry with its
/// argument, then ends the thread.
extern "C" fn thread_start(start: u64) -> ! {
    // SAFETY: `start` is the address of the `Start` at the head of the
    // ThreadStack that `spawn` filled in before creating this thread, and
    // which lives in a static (it is never written again).
    let start = unsafe { &*(start as *const Start) };
    (start.entry)(start.arg);
    thread_exit()
}

/// Starts a thread of this process running `entry(arg)` on `stack`.
/// Returns its thread id, or `InvalidArgument` if the stack was already
/// used.
pub fn spawn<const N: usize>(
    entry: fn(u64),
    arg: u64,
    stack: &'static ThreadStack<N>,
) -> Result<u64, Error> {
    if stack.taken.swap(true, Ordering::AcqRel) {
        return Err(Error::InvalidArgument);
    }
    // SAFETY: winning `taken` makes this the only writer, and no thread
    // reads `start` before the ThreadCreate below.
    unsafe { *stack.start.get() = Start { entry, arg } };
    let top = stack.memory.get() as u64 + N as u64;
    syscall3(
        abi::Syscall::ThreadCreate,
        thread_start as *const () as u64,
        top,
        stack as *const ThreadStack<N> as u64,
    )
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    debug_println!("panic: {}", info);
    exit(101)
}
