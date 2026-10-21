// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Linux system-call dispatcher and the calls that are not about files
//! or memory: process, time, identity, signals (recorded only), terminal
//! control, the thread pointer.

use super::errno::{self, EBADF, EFAULT, EINTR, EINVAL, ENOSYS, ENOTTY, EPERM, ESRCH};
use super::stack::{self, at};
use super::state::OpenFile;
use super::{files, futex, memory, nr, poll, process as lproc, signals, socket};
use crate::task::{self, Process};
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
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
        nr::EXIT => lproc::exit_thread(process, a0),
        nr::EXIT_GROUP => {
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
        nr::FACCESSAT | nr::FACCESSAT2 => files::access(p, a0, a1, a2),
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
        // Only /proc's links (P12).
        nr::READLINK => files::readlinkat(p, files::AT_FDCWD, a0, a1, a2),
        nr::READLINKAT => files::readlinkat(p, a0, a1, a2, a3),
        nr::PREAD64 => files::pread(p, a0, a1, a2, a3),
        nr::PWRITE64 => files::pwrite(p, a0, a1, a2, a3),
        nr::FTRUNCATE => files::ftruncate(p, a0, a1),
        nr::TRUNCATE => files::truncate(p, a0, a1),
        nr::FCHDIR => files::fchdir(p, a0),
        nr::DUP3 => files::dup3(p, a0, a1, a2),
        nr::FSYNC | nr::FDATASYNC => file_of(p, a0).map(|_| 0),
        nr::SYNC | nr::SYNCFS => Ok(0),
        // No modes, owners or settable times are kept (P12).
        nr::CHMOD | nr::CHOWN | nr::LCHOWN => files::exists(p, files::AT_FDCWD, a0),
        nr::FCHMODAT | nr::FCHOWNAT => files::exists(p, a0, a1),
        nr::FCHMOD | nr::FCHOWN => file_of(p, a0).map(|_| 0),
        nr::UTIMENSAT => files::exists(p, a0, a1),
        nr::UTIME | nr::UTIMES => files::exists(p, files::AT_FDCWD, a0),
        nr::UMASK => Ok(p.linux().map_or(0o022, |l| {
            u64::from(l.umask.swap(a0 as u32 & 0o777, Ordering::Relaxed))
        })),
        nr::GETRLIMIT => prlimit(p, a0, a1),
        nr::SETRLIMIT => rlimit(a0).map(|_| 0),
        nr::PRLIMIT64 => prlimit(p, a1, a3),
        nr::SYSINFO => sysinfo(p, a0),
        nr::TIMES => times(p, a0),
        nr::GETRUSAGE => write_user(p, a1, &[0u8; 144]).map(|_| 0),
        nr::IOCTL => ioctl(p, a0, a1, a2),
        nr::BRK => Ok(memory::brk(p, a0)),
        nr::MMAP => memory::mmap(p, a0, a1, a2, a3, a4, a5),
        nr::MUNMAP => memory::munmap(p, a0, a1),
        nr::MPROTECT => memory::mprotect(p, a0, a1, a2),
        nr::MADVISE => Ok(0),
        nr::RT_SIGACTION => signals::rt_sigaction(p, a0, a1, a2),
        nr::RT_SIGPROCMASK => signals::rt_sigprocmask(p, a0, a1, a2),
        nr::RT_SIGRETURN => signals::rt_sigreturn(p, frame),
        nr::RT_SIGPENDING => signals::rt_sigpending(p, a0),
        nr::RT_SIGSUSPEND => signals::rt_sigsuspend(p, a0),
        nr::SIGALTSTACK => signals::sigaltstack(p, a0, a1),
        nr::PAUSE => signals::pause(),
        nr::KILL => signals::kill(p, a0 as i32 as i64, a1),
        nr::TKILL => signals::tgkill(p, 0, a0, a1),
        nr::TGKILL => signals::tgkill(p, a0, a1, a2),
        nr::ALARM => signals::alarm(p, a0),
        nr::SETITIMER => signals::setitimer(p, a0, a1, a2),
        nr::GETITIMER => signals::getitimer(p, a0, a1),
        nr::NANOSLEEP => nanosleep(p, a0, a1),
        nr::CLOCK_NANOSLEEP => clock_nanosleep(p, a0, a1, a2, a3),
        nr::CLOCK_GETTIME => clock_gettime(p, a0, a1),
        nr::GETTIMEOFDAY => gettimeofday(p, a0),
        nr::GETPID => Ok(p.id()),
        nr::GETTID => Ok(task::current_linux().map_or(p.id(), |t| t.tid)),
        nr::GETPPID => Ok(p.linux().map_or(1, |l| l.parent.load(Ordering::Relaxed))),
        nr::GETUID | nr::GETGID | nr::GETEUID | nr::GETEGID => Ok(0),
        nr::SETUID | nr::SETGID => Ok(0),
        nr::GETGROUPS => Ok(0),
        nr::SETPGID => setpgid(p, a0, a1),
        nr::GETPGID => getpgid(p, a0),
        nr::GETPGRP => getpgid(p, 0),
        nr::SETSID => setsid(p),
        nr::GETSID => Ok(p.linux().map_or(p.id(), |l| l.sid.load(Ordering::Relaxed))),
        nr::UNAME => uname(p, a0),
        nr::ARCH_PRCTL => arch_prctl(p, a0, a1),
        nr::SET_TID_ADDRESS => {
            let t = task::current_linux();
            if let Some(t) = &t {
                t.clear_child_tid.store(a0, Ordering::Relaxed);
            }
            Ok(t.map_or(p.id(), |t| t.tid))
        }
        nr::SET_ROBUST_LIST | nr::GET_ROBUST_LIST => Ok(0),
        nr::FUTEX => futex::futex(p, [a0, a1, a2, a3, a4, a5]),
        nr::SCHED_YIELD => {
            task::yield_now();
            Ok(0)
        }
        nr::SCHED_GETAFFINITY => sched_getaffinity(p, a1, a2),
        nr::SCHED_SETAFFINITY | nr::SCHED_SETSCHEDULER => Ok(0),
        nr::SCHED_GETSCHEDULER => Ok(0),
        nr::SCHED_GETPARAM => write_user(p, a1, &[0u8; 4]).map(|_| 0),
        nr::GETCPU => getcpu(p, a0, a1),
        nr::PRCTL => prctl(p, a0, a1),
        nr::CLONE => lproc::clone(&process, frame, a0, a1, a2, a3, a4),
        nr::CLONE3 => lproc::clone3(&process, frame, a0, a1),
        nr::FORK | nr::VFORK => lproc::fork(&process, frame, 17, 0, 0, 0, 0),
        nr::EXECVE => lproc::execve(&process, frame, files::AT_FDCWD, a0, a1, a2, 0),
        nr::EXECVEAT => lproc::execve(&process, frame, a0, a1, a2, a3, a4),
        nr::WAIT4 => lproc::wait4(p, a0 as i32 as i64, a1, a2, a3),
        nr::WAITID => lproc::waitid(p, a0, a1, a2, a3),
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
        nr::PIPE => files::pipe2(p, a0, 0),
        nr::PIPE2 => files::pipe2(p, a0, a1),
        // Probed by musl (pthread_getattr_np) and Rust's std: "no".
        nr::MREMAP | nr::RSEQ | nr::MEMBARRIER | nr::STATX => Err(ENOSYS),
        // No links of either kind.
        nr::SYMLINK | nr::SYMLINKAT | nr::LINK | nr::LINKAT => Err(EPERM),
        _ => unknown(p, number),
    };
    let result = signals::interrupted(number, result);
    drop(process);
    frame.set_rax(errno::encode(result));
}

/// `sched_getaffinity(pid, len, mask)`: the one processor the process
/// runs on (threads never move, `docs/smp.md`).
fn sched_getaffinity(p: &Process, len: u64, mask: u64) -> LinuxResult {
    if len < 8 || !len.is_multiple_of(8) {
        return Err(EINVAL);
    }
    let cpu = p.cpu() as u64;
    write_user(p, mask, &(1u64 << (cpu % 64)).to_le_bytes())?;
    Ok(8)
}

/// `getcpu(cpu, node, cache)`.
fn getcpu(p: &Process, cpu: u64, node: u64) -> LinuxResult {
    if cpu != 0 {
        write_user(p, cpu, &(p.cpu() as u32).to_le_bytes())?;
    }
    if node != 0 {
        write_user(p, node, &0u32.to_le_bytes())?;
    }
    Ok(0)
}

/// `prctl(option, arg2, ...)`: the thread name requests do nothing.
fn prctl(p: &Process, option: u64, arg: u64) -> LinuxResult {
    const PR_SET_NAME: u64 = 15;
    const PR_GET_NAME: u64 = 16;
    match option {
        PR_SET_NAME => Ok(0),
        PR_GET_NAME => {
            let mut name = [0u8; 16];
            let n = p.name().len().min(15);
            name[..n].copy_from_slice(&p.name().as_bytes()[..n]);
            write_user(p, arg, &name)?;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

/// The Linux process `pid` (0: the caller) if it is the caller or one of
/// its children.
fn self_or_child(p: &Process, pid: u64) -> Result<Option<Arc<Process>>, i64> {
    if pid == 0 || pid == p.id() {
        return Ok(None);
    }
    let linux = p.linux().ok_or(ESRCH)?;
    if !linux.children.lock().iter().any(|c| c.pid == pid) {
        return Err(ESRCH);
    }
    task::process::find(pid).map(Some).ok_or(ESRCH)
}

/// `setpgid(pid, pgid)`: the caller or a child joins (or starts) a
/// process group.
fn setpgid(p: &Process, pid: u64, pgid: u64) -> LinuxResult {
    let target = self_or_child(p, pid)?;
    let id = target.as_ref().map_or(p.id(), |t| t.id());
    let pgid = if pgid == 0 { id } else { pgid };
    let linux = match &target {
        Some(t) => t.linux(),
        None => p.linux(),
    }
    .ok_or(ESRCH)?;
    if linux.sid.load(Ordering::Relaxed) == id {
        // A session leader stays in its group.
        return Err(EPERM);
    }
    linux.pgid.store(pgid, Ordering::Relaxed);
    if target.is_some()
        && let Some(mine) = p.linux()
        && let Some(c) = mine.children.lock().iter_mut().find(|c| c.pid == id)
    {
        c.pgid = pgid;
    }
    Ok(0)
}

/// `getpgid(pid)`.
fn getpgid(p: &Process, pid: u64) -> LinuxResult {
    let target = if pid == 0 || pid == p.id() {
        None
    } else {
        Some(task::process::find(pid).ok_or(ESRCH)?)
    };
    let linux = match &target {
        Some(t) => t.linux(),
        None => p.linux(),
    }
    .ok_or(ESRCH)?;
    Ok(linux.pgid.load(Ordering::Relaxed))
}

/// `setsid()`: a new session and process group led by the caller.
fn setsid(p: &Process) -> LinuxResult {
    let linux = p.linux().ok_or(EPERM)?;
    if linux.pgid.load(Ordering::Relaxed) == p.id() && linux.sid.load(Ordering::Relaxed) == p.id() {
        return Err(EPERM);
    }
    linux.pgid.store(p.id(), Ordering::Relaxed);
    linux.sid.store(p.id(), Ordering::Relaxed);
    Ok(p.id())
}

/// The initial stack of a Linux program: its arguments, the environment
/// (`env`, or a few standard variables) and the auxiliary vector.
pub fn initial_stack(
    elf: &Elf,
    argv: &[Vec<u8>],
    env: Option<&[Vec<u8>]>,
    state: Option<&super::State>,
) -> stack::Image {
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
    // The local time zone for the C library (P11 K40, docs/time.md §5.6).
    let mut tz = [0u8; 12];
    let n = abi::time::posix_tz(timezone_minutes(), &mut tz);
    let mut tz_env = Vec::from(&b"TZ="[..]);
    tz_env.extend_from_slice(&tz[..n]);
    let standard: [&[u8]; 5] = [
        b"HOME=/Data",
        b"PATH=/Data/Programs/bin:/Initrd/linux:/Initrd",
        b"TERM=dumb",
        pwd.as_bytes(),
        &tz_env,
    ];
    let env: Vec<&[u8]> = match env {
        Some(e) => e.iter().map(Vec::as_slice).collect(),
        None => standard.to_vec(),
    };
    let args: Vec<&[u8]> = argv.iter().map(Vec::as_slice).collect();
    let mut random = [0u8; 16];
    fill_random(&mut random);
    stack::build(abi::layout::STACK_TOP, &args, &env, &aux, random)
}

/// Minutes east of UTC from `Data:/.stela/timezone` (+08:00 without it).
fn timezone_minutes() -> i32 {
    let read = || -> Option<i32> {
        let v = crate::vfs::find("Data")?;
        let dir = v.fs.lookup(v.fs.root(), ".stela").ok()?;
        let file = v.fs.lookup(dir.id, "timezone").ok()?;
        let mut buf = [0u8; 32];
        let n = v.fs.read(file.id, 0, &mut buf).ok()?;
        abi::time::parse_offset(core::str::from_utf8(&buf[..n]).ok()?)
    };
    read().unwrap_or(abi::time::DEFAULT_OFFSET_MINUTES)
}

/// Random bytes from the kernel's generator (`frame::random`: ChaCha20
/// keyed from RDSEED/RDRAND or TSC jitter; P12).
fn fill_random(buf: &mut [u8]) {
    frame::random::fill(buf);
}

/// `getrandom(buf, len, flags)`: at most 1 MiB at a time.
fn getrandom(p: &Process, buf: u64, len: u64) -> LinuxResult {
    let len = len.min(1 << 20) as usize;
    let mut data = vec![0u8; len];
    fill_random(&mut data);
    write_user(p, buf, &data)?;
    Ok(len as u64)
}

/// `sysinfo(info)`: uptime, memory, process count (`struct sysinfo`, 112
/// bytes, `mem_unit` 1).
fn sysinfo(p: &Process, buf: u64) -> LinuxResult {
    let (free, total) = frame::mm::pmm::stats();
    let mut b = [0u8; 112];
    let put =
        |b: &mut [u8; 112], at: usize, v: u64| b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    put(&mut b, 0, frame::time::now_ns() / 1_000_000_000);
    put(&mut b, 32, total as u64 * 4096);
    put(&mut b, 40, free as u64 * 4096);
    let procs = task::process::list()
        .iter()
        .filter(|(.., code)| code.is_none())
        .count() as u16;
    b[80..82].copy_from_slice(&procs.to_le_bytes());
    b[104..108].copy_from_slice(&1u32.to_le_bytes());
    write_user(p, buf, &b)?;
    Ok(0)
}

/// Clock ticks per second of `times` (`sysconf(_SC_CLK_TCK)` is 100).
const CLK_TCK: u64 = 100;

/// `times(buf)`: no CPU accounting (zeros); returns ticks since boot.
fn times(p: &Process, buf: u64) -> LinuxResult {
    if buf != 0 {
        write_user(p, buf, &[0u8; 32])?;
    }
    Ok(frame::time::now_ns() / (1_000_000_000 / CLK_TCK))
}

/// The fixed resource limits (`getrlimit`, `prlimit64`): (soft, hard).
fn rlimit(resource: u64) -> Result<(u64, u64), i64> {
    const INFINITY: u64 = u64::MAX;
    match resource {
        // RLIMIT_STACK: the 1 MiB stack.
        3 => Ok((abi::layout::STACK_SIZE, abi::layout::STACK_SIZE)),
        // RLIMIT_NOFILE: the descriptor table.
        7 => Ok((super::fd::FD_MAX as u64, super::fd::FD_MAX as u64)),
        // RLIMIT_NPROC.
        6 => Ok((1024, 1024)),
        0..=15 => Ok((INFINITY, INFINITY)),
        _ => Err(EINVAL),
    }
}

/// `prlimit64(pid, resource, new, old)` (and `getrlimit`, `setrlimit`):
/// the limits are fixed; setting one is accepted and ignored.
fn prlimit(p: &Process, resource: u64, old: u64) -> LinuxResult {
    let (soft, hard) = rlimit(resource)?;
    if old != 0 {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&soft.to_le_bytes());
        b[8..].copy_from_slice(&hard.to_le_bytes());
        write_user(p, old, &b)?;
    }
    Ok(0)
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

/// Sleeps until `deadline` (`now_ns`) or a signal; `EINTR` with the time
/// left in `rem` (when given).
fn sleep_until(p: &Process, deadline: u64, rem: u64) -> LinuxResult {
    loop {
        let now = frame::time::now_ns();
        if now >= deadline {
            return Ok(0);
        }
        if task::current_killed() {
            if rem != 0 {
                put_timespec(p, rem, deadline - now)?;
            }
            return Err(EINTR);
        }
        task::block_until(Some(deadline));
    }
}

/// `nanosleep(req, rem)`.
fn nanosleep(p: &Process, req: u64, rem: u64) -> LinuxResult {
    let ns = timespec(p, req)?;
    // At least the requested time, as `sleep_ns` (whole ticks).
    let deadline = frame::time::now_ns()
        .saturating_add(ns)
        .saturating_add(tick_lag());
    sleep_until(p, deadline, rem)
}

/// What the clock may lag behind real time (one tick without an
/// invariant TSC).
fn tick_lag() -> u64 {
    if frame::time::tsc_invariant() {
        0
    } else {
        1_000_000_000 / frame::time::TICK_HZ
    }
}

/// `clock_nanosleep(clock, flags, req, rem)`.
fn clock_nanosleep(p: &Process, clock: u64, flags: u64, req: u64, rem: u64) -> LinuxResult {
    const TIMER_ABSTIME: u64 = 1;
    let ns = timespec(p, req)?;
    let now = clock_ns(clock)?;
    let wait = if flags & TIMER_ABSTIME != 0 {
        ns.saturating_sub(now)
    } else {
        ns
    };
    let deadline = frame::time::now_ns()
        .saturating_add(wait)
        .saturating_add(tick_lag());
    let rem = if flags & TIMER_ABSTIME != 0 { 0 } else { rem };
    sleep_until(p, deadline, rem)
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
    const TIOCSWINSZ: u64 = 0x5414;
    const TIOCSCTTY: u64 = 0x540E;
    const TIOCNOTTY: u64 = 0x5422;
    const FIONREAD: u64 = 0x541B;
    const FIONBIO: u64 = 0x5421;
    const TIOCGPTN: u64 = 0x8004_5430;
    const TIOCSPTLCK: u64 = 0x4004_5431;
    let file = file_of(p, fd)?;
    if request == FIONREAD {
        let n = files::readable_bytes(&file) as u32;
        write_user(p, arg, &n.to_le_bytes())?;
        return Ok(0);
    }
    if request == FIONBIO {
        let on = read_user(p, arg, 4)? != [0, 0, 0, 0];
        match &*file {
            OpenFile::PipeRead(_, nb)
            | OpenFile::PipeWrite(_, nb)
            | OpenFile::PtyMaster(_, _, nb) => {
                nb.store(on, Ordering::Relaxed);
            }
            _ => {}
        }
        return Ok(0);
    }
    let tty = match &*file {
        OpenFile::Tty(tty) => tty,
        OpenFile::PtyMaster(tty, n, _) => match request {
            TIOCGPTN => {
                write_user(p, arg, &n.to_le_bytes())?;
                return Ok(0);
            }
            TIOCSPTLCK => return Ok(0),
            _ => tty,
        },
        _ => return Err(ENOTTY),
    };
    match request {
        TIOCSWINSZ => {
            let w = read_user(p, arg, 8)?;
            let rows = u16::from_le_bytes([w[0], w[1]]);
            let cols = u16::from_le_bytes([w[2], w[3]]);
            tty.set_size(cols, rows);
            Ok(0)
        }
        TIOCSCTTY => {
            tty.add_user(p.id());
            Ok(0)
        }
        TIOCNOTTY => Ok(0),
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
            // Rows, columns (what the terminal set; 25 x 80 at first), no
            // pixel sizes.
            let (cols, rows) = tty.size();
            let mut w = [0u8; 8];
            w[0..2].copy_from_slice(&rows.to_le_bytes());
            w[2..4].copy_from_slice(&cols.to_le_bytes());
            write_user(p, arg, &w)?;
            Ok(0)
        }
        TIOCGPGRP => {
            let pgid = tty
                .foreground()
                .unwrap_or_else(|| p.linux().map_or(p.id(), |l| l.pgid.load(Ordering::Relaxed)));
            write_user(p, arg, &(pgid as u32).to_le_bytes())?;
            Ok(0)
        }
        TIOCSPGRP => {
            let b = read_user(p, arg, 4)?;
            let pgid = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            tty.set_foreground(Some(u64::from(pgid)));
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}
