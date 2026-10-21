// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Linux system-call dispatcher and the calls that are not about files
//! or memory: process, time, identity, signals (recorded only), terminal
//! control, the thread pointer.

use super::errno::{self, EAGAIN, EBADF, EFAULT, EINVAL, ENOSYS, ENOTTY};
use super::stack::{self, at};
use super::state::OpenFile;
use super::{files, memory, nr, poll, socket};
use crate::task::{self, Process};
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use elfload::Elf;
use frame::sync::SpinLock;
use frame::syscall::SyscallFrame;

/// A Linux call's result: the value, or the errno.
pub type LinuxResult = Result<u64, i64>;

/// Copies `len` bytes of the caller's memory at `ptr`.
pub fn read_user(process: &Process, ptr: u64, len: usize) -> Result<Vec<u8>, i64> {
    let mut buf = vec![0u8; len];
    process.space().read(ptr, &mut buf).map_err(|_| EFAULT)?;
    Ok(buf)
}

/// Copies `data` into the caller's memory at `ptr`.
pub fn write_user(process: &Process, ptr: u64, data: &[u8]) -> Result<(), i64> {
    process.space().write(ptr, data).map_err(|_| EFAULT)
}

/// Reads a `u64` of the caller's memory.
pub fn read_u64(process: &Process, ptr: u64) -> Result<u64, i64> {
    let b = read_user(process, ptr, 8)?;
    let mut word = [0u8; 8];
    word.copy_from_slice(&b);
    Ok(u64::from_le_bytes(word))
}

/// The descriptor's open file.
pub fn file_of(process: &Process, fd: u64) -> Result<Arc<OpenFile>, i64> {
    let linux = process.linux().ok_or(EBADF)?;
    let fds = linux.fds.lock();
    fds.get(fd).map(|s| Arc::clone(&s.file)).map_err(|_| EBADF)
}

/// The numbers already reported as unknown (each is reported once).
static WARNED: SpinLock<BTreeSet<u64>> = SpinLock::new(BTreeSet::new());

fn unknown(process: &Process, number: u64) -> LinuxResult {
    if WARNED.lock().insert(number) {
        frame::kprintln!(
            "linux: process {} ({}): system call {number} is not supported (ENOSYS)",
            process.id(),
            process.name()
        );
    }
    Err(ENOSYS)
}

/// Handles a system call of a Linux process. Takes the process by value:
/// `exit` never returns and must not leave a reference behind on this
/// stack.
pub fn dispatch(frame: &mut SyscallFrame, process: Arc<Process>) {
    let number = frame.number();
    let [a0, a1, a2, a3, a4, a5] = frame.args();
    let p = &*process;
    let result = match number {
        nr::EXIT | nr::EXIT_GROUP => {
            drop(process);
            task::exit_process(a0 as i64 & 0xFF)
        }
        nr::READ => files::read(p, a0, a1, a2),
        nr::WRITE => files::write(p, a0, a1, a2),
        nr::READV => files::readv(p, a0, a1, a2),
        nr::WRITEV => files::writev(p, a0, a1, a2),
        nr::OPEN => files::openat(p, files::AT_FDCWD, a0, a1),
        nr::OPENAT => files::openat(p, a0, a1, a2),
        nr::CLOSE => files::close(p, a0),
        nr::STAT | nr::LSTAT => files::stat_path(p, files::AT_FDCWD, a0, a1, 0),
        nr::NEWFSTATAT => files::stat_path(p, a0, a1, a2, a3),
        nr::FSTAT => files::fstat(p, a0, a1),
        nr::LSEEK => files::lseek(p, a0, a1 as i64, a2),
        nr::ACCESS => files::access(p, files::AT_FDCWD, a0, a1),
        nr::FACCESSAT => files::access(p, a0, a1, a2),
        nr::DUP => files::dup(p, a0),
        nr::DUP2 => files::dup2(p, a0, a1),
        nr::FCNTL => files::fcntl(p, a0, a1, a2),
        nr::GETCWD => files::getcwd(p, a0, a1),
        nr::CHDIR => files::chdir(p, a0),
        nr::RENAME => files::renameat(p, files::AT_FDCWD, a0, files::AT_FDCWD, a1),
        nr::RENAMEAT => files::renameat(p, a0, a1, a2, a3),
        nr::MKDIR => files::mkdirat(p, files::AT_FDCWD, a0),
        nr::MKDIRAT => files::mkdirat(p, a0, a1),
        nr::RMDIR => files::unlinkat(p, files::AT_FDCWD, a0, files::AT_REMOVEDIR),
        nr::UNLINK => files::unlinkat(p, files::AT_FDCWD, a0, 0),
        nr::UNLINKAT => files::unlinkat(p, a0, a1, a2),
        nr::GETDENTS64 => files::getdents64(p, a0, a1, a2),
        nr::POLL => poll::poll(p, a0, a1, a2),
        nr::PPOLL => poll::ppoll(p, a0, a1, a2),
        nr::SELECT => poll::select(p, [a0, a1, a2, a3, a4]),
        nr::PSELECT6 => poll::pselect6(p, [a0, a1, a2, a3, a4]),
        // No symbolic links anywhere.
        nr::READLINK | nr::READLINKAT => Err(errno::ENOENT),
        nr::IOCTL => ioctl(p, a0, a1, a2),
        nr::BRK => Ok(memory::brk(p, a0)),
        nr::MMAP => memory::mmap(p, a0, a1, a2, a3, a4, a5),
        nr::MUNMAP => memory::munmap(p, a0, a1),
        nr::MPROTECT => memory::mprotect(p, a0, a1, a2),
        nr::RT_SIGACTION => rt_sigaction(p, a2),
        nr::RT_SIGPROCMASK => rt_sigprocmask(p, a2),
        nr::NANOSLEEP => nanosleep(p, a0),
        nr::CLOCK_NANOSLEEP => clock_nanosleep(p, a0, a1, a2),
        nr::CLOCK_GETTIME => clock_gettime(p, a0, a1),
        nr::GETTIMEOFDAY => gettimeofday(p, a0),
        nr::GETPID | nr::GETTID => Ok(p.id()),
        // The parent is not tracked; init's id stands in for it.
        nr::GETPPID => Ok(1),
        nr::GETUID | nr::GETGID | nr::GETEUID | nr::GETEGID => Ok(0),
        nr::UNAME => uname(p, a0),
        nr::ARCH_PRCTL => arch_prctl(p, a0, a1),
        nr::SET_TID_ADDRESS => {
            if let Some(linux) = p.linux() {
                linux.tid_address.store(a0, Ordering::Relaxed);
            }
            Ok(p.id())
        }
        nr::FUTEX => futex(p, a0, a1, a2, a3),
        nr::GETRANDOM => getrandom(p, a0, a1),
        nr::SOCKET => socket::socket(p, a0, a1, a2),
        nr::CONNECT => socket::connect(p, a0, a1, a2),
        nr::BIND => socket::bind(p, a0, a1, a2),
        nr::LISTEN => socket::listen(p, a0, a1),
        nr::ACCEPT => socket::accept(p, a0, a1, a2, 0),
        nr::ACCEPT4 => socket::accept(p, a0, a1, a2, a3),
        nr::SENDTO => socket::sendto(p, a0, a1, a2, a4, a5),
        nr::RECVFROM => socket::recvfrom(p, [a0, a1, a2, a3, a4, a5]),
        nr::SENDMSG => socket::sendmsg(p, a0, a1),
        nr::RECVMSG => socket::recvmsg(p, a0, a1, a2),
        nr::SHUTDOWN => socket::shutdown(p, a0, a1),
        nr::GETSOCKNAME => socket::name(p, a0, a1, a2, false),
        nr::GETPEERNAME => socket::name(p, a0, a1, a2, true),
        nr::SETSOCKOPT => socket::setsockopt(p, a0),
        nr::GETSOCKOPT => socket::getsockopt(p, a0, a3, a4),
        nr::PIPE | nr::PIPE2 | nr::CLONE | nr::CLONE3 | nr::FORK | nr::VFORK | nr::EXECVE => {
            Err(ENOSYS)
        }
        _ => unknown(p, number),
    };
    drop(process);
    frame.set_rax(errno::encode(result));
}

/// The initial stack of a Linux program: its arguments, a few environment
/// variables and the auxiliary vector.
pub fn initial_stack(elf: &Elf, argv: &[Vec<u8>], state: Option<&super::State>) -> stack::Image {
    let (phnum, phent) = elf.program_headers();
    let aux = [
        (at::PHDR, elf.program_headers_address().unwrap_or(0)),
        (at::PHENT, phent as u64),
        (at::PHNUM, phnum as u64),
        (at::PAGESZ, 4096),
        (at::ENTRY, elf.entry()),
        (at::UID, 0),
        (at::EUID, 0),
        (at::GID, 0),
        (at::EGID, 0),
        (at::SECURE, 0),
    ];
    let cwd = state.map_or_else(
        || alloc::string::String::from("/"),
        |s| s.cwd.lock().clone(),
    );
    let pwd = alloc::format!("PWD={cwd}");
    let env: [&[u8]; 4] = [
        b"HOME=/Data",
        b"PATH=/Initrd/linux:/Initrd",
        b"TERM=dumb",
        pwd.as_bytes(),
    ];
    let args: Vec<&[u8]> = argv.iter().map(Vec::as_slice).collect();
    let mut random = [0u8; 16];
    fill_random(&mut random);
    stack::build(abi::layout::STACK_TOP, &args, &env, &aux, random)
}

/// The generator state of [`fill_random`].
static RANDOM: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// Fills `buf` with splitmix64 output seeded from the clocks. **Not
/// cryptographically secure** (`getrandom` and AT_RANDOM use it).
fn fill_random(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(8) {
        let seed = frame::time::now_ns() ^ frame::time::wall_clock_ns().rotate_left(17);
        let mut z = RANDOM
            .fetch_add(0x9E37_79B9_7F4A_7C15 ^ seed, Ordering::Relaxed)
            .wrapping_add(seed);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
    }
}

/// `getrandom(buf, len, flags)`.
fn getrandom(p: &Process, buf: u64, len: u64) -> LinuxResult {
    let len = len.min(256) as usize;
    let mut data = vec![0u8; len];
    fill_random(&mut data);
    write_user(p, buf, &data)?;
    Ok(len as u64)
}

/// `uname(buf)`: six 65-byte fields.
fn uname(p: &Process, buf: u64) -> LinuxResult {
    let fields: [&str; 6] = [
        "STELA",
        "stela",
        env!("CARGO_PKG_VERSION"),
        "STELA (Linux personality)",
        "x86_64",
        "(none)",
    ];
    let mut out = [0u8; 6 * 65];
    for (i, f) in fields.iter().enumerate() {
        out[i * 65..i * 65 + f.len()].copy_from_slice(f.as_bytes());
    }
    write_user(p, buf, &out)?;
    Ok(0)
}

/// `arch_prctl(code, addr)`: the FS base (TLS).
fn arch_prctl(p: &Process, code: u64, addr: u64) -> LinuxResult {
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;
    match code {
        ARCH_SET_FS => frame::task::set_fs_base(addr)
            .map(|_| 0)
            .map_err(|_| EINVAL),
        ARCH_GET_FS => {
            let base = frame::task::current().fs_base();
            write_user(p, addr, &base.to_le_bytes())?;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

/// `rt_sigaction(sig, act, oldact)`: signals are never delivered; the old
/// action reads as the default.
fn rt_sigaction(p: &Process, old: u64) -> LinuxResult {
    if old != 0 {
        write_user(p, old, &[0u8; 32])?;
    }
    Ok(0)
}

/// `rt_sigprocmask(how, set, oldset)`: recorded nowhere; the old mask is
/// empty.
fn rt_sigprocmask(p: &Process, old: u64) -> LinuxResult {
    if old != 0 {
        write_user(p, old, &[0u8; 8])?;
    }
    Ok(0)
}

/// A `struct timespec` of the caller: nanoseconds.
fn timespec(p: &Process, ptr: u64) -> Result<u64, i64> {
    let sec = read_u64(p, ptr)? as i64;
    let nsec = read_u64(p, ptr + 8)? as i64;
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(EINVAL);
    }
    Ok((sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64))
}

fn put_timespec(p: &Process, ptr: u64, ns: u64) -> Result<(), i64> {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&(ns / 1_000_000_000).to_le_bytes());
    b[8..].copy_from_slice(&(ns % 1_000_000_000).to_le_bytes());
    write_user(p, ptr, &b)
}

const CLOCK_REALTIME: u64 = 0;
const CLOCK_REALTIME_COARSE: u64 = 5;

/// The time on clock `clock` in ns (REALTIME: since 1970; the others:
/// since boot).
fn clock_ns(clock: u64) -> Result<u64, i64> {
    match clock {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => Ok(frame::time::wall_clock_ns()),
        // MONOTONIC, PROCESS/THREAD_CPUTIME (approximated), MONOTONIC_RAW,
        // MONOTONIC_COARSE, BOOTTIME.
        1..=4 | 6 | 7 => Ok(frame::time::now_ns()),
        _ => Err(EINVAL),
    }
}

/// `clock_gettime(clock, ts)`.
fn clock_gettime(p: &Process, clock: u64, ts: u64) -> LinuxResult {
    put_timespec(p, ts, clock_ns(clock)?)?;
    Ok(0)
}

/// `gettimeofday(tv, tz)`.
fn gettimeofday(p: &Process, tv: u64) -> LinuxResult {
    if tv != 0 {
        let ns = frame::time::wall_clock_ns();
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&(ns / 1_000_000_000).to_le_bytes());
        b[8..].copy_from_slice(&(ns / 1000 % 1_000_000).to_le_bytes());
        write_user(p, tv, &b)?;
    }
    Ok(0)
}

/// `nanosleep(req, rem)`: never interrupted, so `rem` is not written.
fn nanosleep(p: &Process, req: u64) -> LinuxResult {
    let ns = timespec(p, req)?;
    task::sleep_ns(ns);
    Ok(0)
}

/// `clock_nanosleep(clock, flags, req, rem)`.
fn clock_nanosleep(p: &Process, clock: u64, flags: u64, req: u64) -> LinuxResult {
    const TIMER_ABSTIME: u64 = 1;
    let ns = timespec(p, req)?;
    let now = clock_ns(clock)?;
    let wait = if flags & TIMER_ABSTIME != 0 {
        ns.saturating_sub(now)
    } else {
        ns
    };
    task::sleep_ns(wait);
    Ok(0)
}

/// `futex(uaddr, op, val, timeout)`: a single-threaded process has nobody
/// to wake it, so `FUTEX_WAIT` returns `EAGAIN` when the value differs and
/// otherwise sleeps out its timeout (or until killed); `FUTEX_WAKE` wakes
/// nobody.
fn futex(p: &Process, uaddr: u64, op: u64, val: u64, timeout: u64) -> LinuxResult {
    const FUTEX_WAIT: u64 = 0;
    const FUTEX_WAKE: u64 = 1;
    const FUTEX_CMD_MASK: u64 = 0x7F;
    const ETIMEDOUT: i64 = errno::ETIMEDOUT;
    match op & FUTEX_CMD_MASK {
        FUTEX_WAIT => {
            let current = read_user(p, uaddr, 4)?;
            let current = u32::from_le_bytes([current[0], current[1], current[2], current[3]]);
            if current != val as u32 {
                return Err(EAGAIN);
            }
            if timeout == 0 {
                while !task::current_killed() {
                    task::sleep_ms(1000);
                }
                return Err(errno::EINTR);
            }
            task::sleep_ns(timespec(p, timeout)?);
            Err(ETIMEDOUT)
        }
        FUTEX_WAKE => Ok(0),
        _ => Err(ENOSYS),
    }
}

/// termios `c_lflag` bits (`include/uapi/asm-generic/termbits.h`).
const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
/// Size of the kernel's `struct termios` (TCGETS): four flag words, the
/// line discipline byte and 19 control characters.
const TERMIOS_SIZE: usize = 36;

/// `ioctl(fd, request, arg)`: terminal requests on ttys, `ENOTTY`
/// otherwise.
fn ioctl(p: &Process, fd: u64, request: u64, arg: u64) -> LinuxResult {
    const TCGETS: u64 = 0x5401;
    const TCSETS: u64 = 0x5402;
    const TCSETSW: u64 = 0x5403;
    const TCSETSF: u64 = 0x5404;
    const TIOCGPGRP: u64 = 0x540F;
    const TIOCSPGRP: u64 = 0x5410;
    const TIOCGWINSZ: u64 = 0x5413;
    let file = file_of(p, fd)?;
    let OpenFile::Tty(tty) = &*file else {
        return Err(ENOTTY);
    };
    match request {
        TCGETS => {
            let (canonical, echo, signals) = tty.mode();
            let mut lflag = 0;
            if canonical {
                lflag |= ICANON;
            }
            if echo {
                lflag |= ECHO;
            }
            if signals {
                lflag |= ISIG;
            }
            let mut t = [0u8; TERMIOS_SIZE];
            // c_iflag ICRNL, c_oflag OPOST|ONLCR, c_cflag CS8|CREAD.
            t[0..4].copy_from_slice(&0o400u32.to_le_bytes());
            t[4..8].copy_from_slice(&0o5u32.to_le_bytes());
            t[8..12].copy_from_slice(&0o260u32.to_le_bytes());
            t[12..16].copy_from_slice(&lflag.to_le_bytes());
            // c_cc: VINTR ^C, VERASE DEL, VKILL ^U, VEOF ^D, VMIN 1.
            let cc = 17;
            t[cc] = 3;
            t[cc + 2] = 0x7F;
            t[cc + 3] = 0x15;
            t[cc + 4] = 4;
            t[cc + 6] = 1;
            write_user(p, arg, &t)?;
            Ok(0)
        }
        TCSETS | TCSETSW | TCSETSF => {
            let t = read_user(p, arg, TERMIOS_SIZE)?;
            let lflag = u32::from_le_bytes([t[12], t[13], t[14], t[15]]);
            tty.set_mode(lflag & ICANON != 0, lflag & ECHO != 0, lflag & ISIG != 0);
            Ok(0)
        }
        TIOCGWINSZ => {
            // 25 rows, 80 columns, no pixel sizes.
            let mut w = [0u8; 8];
            w[0..2].copy_from_slice(&25u16.to_le_bytes());
            w[2..4].copy_from_slice(&80u16.to_le_bytes());
            write_user(p, arg, &w)?;
            Ok(0)
        }
        TIOCGPGRP => {
            write_user(p, arg, &(p.id() as u32).to_le_bytes())?;
            Ok(0)
        }
        TIOCSPGRP => Ok(0),
        _ => Err(EINVAL),
    }
}
