// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Processes (`docs/userland.md`): starting a program from a file handle
//! with arguments and handles, waiting, killing, and the arguments this
//! program got.

use crate::ipc::{FOREVER, Mapping, handle_close, memory_create, port_receive};
use crate::sys::{syscall0, syscall1, syscall2, syscall3, syscall6};
use abi::capability;
use abi::sysinfo::{self, MemInfo, PROCESS_INFO_SIZE, ProcessInfo};
use abi::{Error, Handle, MESSAGE_DATA, Message, Rights, Syscall, bootstrap, tag};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Starts the program in `file` (a handle with `READ`) with `args` (the
/// program name first) and `handles` as its `[1]`, `[2]`, `[3]`
/// (`Handle::NONE` for none; each is moved and needs `TRANSFER`).
/// Returns a handle with `WAIT | KILL`. The child also gets copies of
/// this program's inherited capabilities ([`inherited`]: `NETWORK`,
/// `INIT`).
pub fn spawn(file: Handle, args: &[&str], handles: [Handle; 3]) -> Result<Handle, Error> {
    spawn_with(file, args, &handles, &inherited())
}

/// Copies of the capabilities a child gets by default: `NETWORK` and
/// `INIT` (`POWER` and `INPUT` are only given on purpose).
pub fn inherited() -> Vec<(u32, Handle)> {
    [capability::NETWORK, capability::INIT]
        .into_iter()
        .filter_map(|kind| pass_capability(kind).map(|h| (kind, h)))
        .collect()
}

/// A copy of this program's capability `kind` to give a child, if it has
/// one.
pub fn pass_capability(kind: u32) -> Option<Handle> {
    let h = capability(kind)?;
    let rights = if kind == capability::INIT {
        Rights::SEND | Rights::DUPLICATE
    } else {
        Rights::DUPLICATE
    };
    let copy = pass(h, rights);
    (copy != Handle::NONE).then_some(copy)
}

/// Like [`spawn`], with up to six initial handles (`[1]`..`[6]`:
/// console, bootstrap, cwd, program folder, stdin, reserved) and exactly
/// `caps` (kind, handle; moved, each needs `TRANSFER`) as the child's
/// capabilities.
pub fn spawn_with(
    file: Handle,
    args: &[&str],
    handles: &[Handle],
    caps: &[(u32, Handle)],
) -> Result<Handle, Error> {
    let r = spawn_raw(file, args, handles, caps);
    if r.is_err() {
        // Not moved: do not keep the copies.
        for &(_, h) in caps {
            let _ = handle_close(h);
        }
    }
    r
}

fn spawn_raw(
    file: Handle,
    args: &[&str],
    handles: &[Handle],
    caps: &[(u32, Handle)],
) -> Result<Handle, Error> {
    if caps.len() > capability::MAX || handles.len() > bootstrap::GIVEN {
        return Err(Error::InvalidArgument);
    }
    let mut pairs = [0u32; 2 * capability::MAX];
    for (i, &(kind, h)) in caps.iter().enumerate() {
        pairs[2 * i] = h.0;
        pairs[2 * i + 1] = kind;
    }
    let mut joined: Vec<u8> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            joined.push(0);
        }
        joined.extend_from_slice(a.as_bytes());
    }
    let msg = if joined.len() <= MESSAGE_DATA {
        Message::with_data(tag::BOOTSTRAP_ARGS, &joined)
    } else {
        // Too long for a message: in shared memory, the length in the data.
        let size = (joined.len() as u64).next_multiple_of(4096);
        let memory = memory_create(size)?;
        {
            let map = Mapping::new(memory, size, true)?;
            map.write(0, &joined)?;
        }
        let mut msg = Message::with_data(tag::BOOTSTRAP_ARGS, &(joined.len() as u64).to_le_bytes());
        msg.push_handle(memory);
        msg
    };
    let raw: [u8; 4 * bootstrap::GIVEN] = {
        let mut r = [0u8; 4 * bootstrap::GIVEN];
        for (i, h) in handles.iter().enumerate() {
            r[4 * i..4 * i + 4].copy_from_slice(&h.0.to_le_bytes());
        }
        r
    };
    syscall6(
        Syscall::ProcessSpawn,
        [
            file.raw(),
            &msg as *const Message as u64,
            raw.as_ptr() as u64,
            handles.len() as u64,
            pairs.as_ptr() as u64,
            caps.len() as u64,
        ],
    )
    .map(Handle::from_raw)
}

/// A `TRANSFER`-able duplicate of `h` with its own rights (to pass to a
/// child), or `Handle::NONE` if `h` is not valid.
pub fn pass(h: Handle, rights: Rights) -> Handle {
    crate::ipc::handle_duplicate(h, rights.union(Rights::TRANSFER)).unwrap_or(Handle::NONE)
}

/// A copy of the folder handle `h` to pass on, with as many of `READ`,
/// `WRITE` and `MANAGE` as it has (`Handle::NONE` if it is not valid).
pub fn pass_dir(h: Handle) -> Handle {
    [
        Rights::READ | Rights::WRITE | Rights::MANAGE,
        Rights::READ | Rights::WRITE,
        Rights::READ,
    ]
    .into_iter()
    .map(|r| pass(h, r | Rights::DUPLICATE))
    .find(|&c| c != Handle::NONE)
    .unwrap_or(Handle::NONE)
}

/// The standard handles for a child: the console and `cwd` (both
/// duplicated), and `bootstrap` as `[2]`.
pub fn standard_handles(bootstrap_handle: Handle, cwd: Handle) -> [Handle; 3] {
    [
        pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
        bootstrap_handle,
        pass_dir(cwd),
    ]
}

/// Waits for a process to end (up to `timeout_ns`); its exit code.
pub fn wait(process: Handle, timeout_ns: u64) -> Result<i64, Error> {
    syscall2(Syscall::ProcessWait, process.raw(), timeout_ns).map(|c| c as i64)
}

/// Waits as long as it takes.
pub fn wait_forever(process: Handle) -> Result<i64, Error> {
    wait(process, FOREVER)
}

/// Ends a process (exit code `abi::EXIT_KILLED`).
pub fn kill(process: Handle) -> Result<(), Error> {
    syscall1(Syscall::ProcessKill, process.raw()).map(|_| ())
}

/// A handle to this process (`KILL`).
pub fn this() -> Result<Handle, Error> {
    syscall0(Syscall::ProcessSelf).map(Handle::from_raw)
}

/// The processes the kernel knows (running and ended).
pub fn list() -> Result<Vec<ProcessInfo>, Error> {
    let mut buf = vec![0u8; 64 * PROCESS_INFO_SIZE];
    loop {
        let n = syscall3(
            Syscall::SysInfo,
            sysinfo::PROCESSES,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )? as usize;
        if n * PROCESS_INFO_SIZE <= buf.len() {
            return Ok(buf
                .as_chunks::<PROCESS_INFO_SIZE>()
                .0
                .iter()
                .take(n)
                .filter_map(|c| ProcessInfo::from_bytes(c))
                .collect());
        }
        buf = vec![0u8; n * PROCESS_INFO_SIZE];
    }
}

/// Memory counters.
pub fn memory() -> Result<MemInfo, Error> {
    let mut b = [0u8; sysinfo::MEM_INFO_SIZE];
    syscall3(
        Syscall::SysInfo,
        sysinfo::MEMORY,
        b.as_mut_ptr() as u64,
        b.len() as u64,
    )?;
    MemInfo::from_bytes(&b).ok_or(Error::InvalidArgument)
}

/// The display, if there is a framebuffer (`SysInfo(FRAMEBUFFER)`).
pub fn framebuffer_info() -> Option<sysinfo::FramebufferInfo> {
    let mut b = [0u8; sysinfo::FRAMEBUFFER_INFO_SIZE];
    match syscall3(
        Syscall::SysInfo,
        sysinfo::FRAMEBUFFER,
        b.as_mut_ptr() as u64,
        b.len() as u64,
    ) {
        Ok(1) => sysinfo::FramebufferInfo::from_bytes(&b),
        _ => None,
    }
}

/// The boot options (`SysInfo(CMDLINE)`): space-separated `key` or
/// `key=value` words; empty when there are none.
pub fn cmdline() -> Vec<u8> {
    let mut b = vec![0u8; sysinfo::CMDLINE_SIZE];
    match syscall3(
        Syscall::SysInfo,
        sysinfo::CMDLINE,
        b.as_mut_ptr() as u64,
        b.len() as u64,
    ) {
        Ok(n) => b.truncate((n as usize).min(sysinfo::CMDLINE_SIZE)),
        Err(_) => b.clear(),
    }
    b
}

/// The hardware report (`SysInfo(HARDWARE)`, P11, `docs/hwinfo.md`):
/// text, one item a line.
pub fn hardware() -> Result<alloc::string::String, Error> {
    let mut size = 16 * 1024;
    loop {
        let mut b = vec![0u8; size];
        let n = syscall3(
            Syscall::SysInfo,
            sysinfo::HARDWARE,
            b.as_mut_ptr() as u64,
            b.len() as u64,
        )? as usize;
        if n <= size {
            b.truncate(n);
            return Ok(alloc::string::String::from_utf8_lossy(&b).into_owned());
        }
        size = n;
    }
}

/// A handle (`WAIT | KILL`) to running process `pid` (`ProcessOpen`,
/// P11): needs the `POWER` capability; `AccessDenied` without it.
pub fn open(pid: u64) -> Result<Handle, Error> {
    let power = capability(capability::POWER).ok_or(Error::AccessDenied)?;
    syscall2(Syscall::ProcessOpen, pid, power.raw()).map(Handle::from_raw)
}

/// The processors: each one's timer ticks (`SysInfo(CPUS)`).
pub fn cpus() -> Vec<u64> {
    let mut b = vec![0u8; 8 * 64];
    let n = syscall3(
        Syscall::SysInfo,
        sysinfo::CPUS,
        b.as_mut_ptr() as u64,
        b.len() as u64,
    )
    .unwrap_or(0) as usize;
    b.as_chunks::<8>()
        .0
        .iter()
        .take(n.min(64))
        .map(|c| u64::from_le_bytes(*c))
        .collect()
}

/// The running processes and the processor each runs on
/// (`SysInfo(PROCESS_CPUS)`).
pub fn process_cpus() -> Vec<(u64, u64)> {
    let mut b = vec![0u8; 16 * 256];
    let n = syscall3(
        Syscall::SysInfo,
        sysinfo::PROCESS_CPUS,
        b.as_mut_ptr() as u64,
        b.len() as u64,
    )
    .unwrap_or(0) as usize;
    b.as_chunks::<16>()
        .0
        .iter()
        .take(n.min(256))
        .map(|c| {
            let mut pid = [0u8; 8];
            let mut cpu = [0u8; 8];
            pid.copy_from_slice(&c[..8]);
            cpu.copy_from_slice(&c[8..]);
            (u64::from_le_bytes(pid), u64::from_le_bytes(cpu))
        })
        .collect()
}

/// The wall clock: nanoseconds since 1970-01-01 00:00 UTC
/// (`SysInfo(TIME)`).
pub fn wall_clock_ns() -> Result<u64, Error> {
    let mut b = [0u8; 8];
    syscall3(Syscall::SysInfo, sysinfo::TIME, b.as_mut_ptr() as u64, 8)?;
    Ok(u64::from_le_bytes(b))
}

/// Whether the boot options set `key` (`sysinfo::cmdline_flag`).
pub fn boot_flag(key: &str) -> bool {
    sysinfo::cmdline_flag(&cmdline(), key)
}

/// Flushes the volumes and turns the machine off. Returns only on
/// failure (`NotSupported` when the machine has no ACPI soft off,
/// `AccessDenied` without a way to do it). See [`power`].
pub fn shutdown() -> Error {
    power(abi::power::SHUTDOWN)
}

/// Flushes the volumes and restarts the machine. Returns only on failure.
pub fn reboot() -> Error {
    power(abi::power::REBOOT)
}

/// How long to wait for init's answer to a `POWER_REQUEST`.
const POWER_TIMEOUT: u64 = 10_000_000_000;

/// `SystemPower(kind)` with the `POWER` capability, or else a
/// `POWER_REQUEST` to init (the `INIT` capability), which does it.
pub fn power(kind: u64) -> Error {
    if let Some(p) = capability(capability::POWER) {
        return match syscall2(Syscall::SystemPower, kind, p.raw()) {
            Ok(_) => Error::NotSupported,
            Err(e) => e,
        };
    }
    let Some(init) = capability(capability::INIT) else {
        return Error::AccessDenied;
    };
    let Ok(reply) = crate::ipc::port_create() else {
        return Error::OutOfHandles;
    };
    let result = (|| {
        let mut msg = Message::with_data(tag::POWER_REQUEST, &kind.to_le_bytes());
        msg.push_handle(crate::ipc::handle_duplicate(
            reply,
            Rights::SEND | Rights::TRANSFER,
        )?);
        crate::ipc::port_send(init, &msg)?;
        let mut answer = Message::new();
        port_receive(reply, &mut answer, POWER_TIMEOUT)?;
        let code = answer
            .payload()
            .get(..8)
            .map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])));
        Err(Error::from_raw(code).unwrap_or(Error::NotSupported))
    })();
    let _ = handle_close(reply);
    match result {
        Ok(()) => Error::NotSupported,
        Err(e) => e,
    }
}

/// (uptime ns, timer ticks).
pub fn uptime() -> Result<(u64, u64), Error> {
    let mut b = [0u8; 16];
    syscall3(Syscall::SysInfo, sysinfo::UPTIME, b.as_mut_ptr() as u64, 16)?;
    let mut ns = [0u8; 8];
    let mut ticks = [0u8; 8];
    ns.copy_from_slice(&b[..8]);
    ticks.copy_from_slice(&b[8..]);
    Ok((u64::from_le_bytes(ns), u64::from_le_bytes(ticks)))
}

static ARGS: crate::sync::SpinMutex<Vec<String>> = crate::sync::SpinMutex::new(Vec::new());

/// Splits NUL-separated arguments.
fn split(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    bytes
        .split(|&b| b == 0)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

/// The capabilities this program holds: (kind, handle).
static CAPS: crate::sync::SpinMutex<Vec<(u32, Handle)>> = crate::sync::SpinMutex::new(Vec::new());

/// This program's capability of `kind` (`abi::capability`), if it was
/// given one.
pub fn capability(kind: u32) -> Option<Handle> {
    CAPS.lock().iter().find(|c| c.0 == kind).map(|c| c.1)
}

/// Records `handle` as this program's capability of `kind` (init makes
/// its own `INIT`), replacing one it had.
pub fn set_capability(kind: u32, handle: Handle) {
    let mut caps = CAPS.lock();
    caps.retain(|c| c.0 != kind);
    caps.push((kind, handle));
}

/// Takes the `BOOTSTRAP_ARGS` and `CAPABILITIES` messages the kernel
/// queued first on the main port. Called by `entry!` before `main`.
#[doc(hidden)]
pub fn take_args() {
    take_arguments();
    let mut msg = Message::new();
    if port_receive(bootstrap::MAIN_PORT, &mut msg, 0).is_err() || msg.tag != tag::CAPABILITIES {
        return;
    }
    let kinds = msg.payload();
    let mut caps = CAPS.lock();
    for (i, &h) in msg.carried().iter().enumerate() {
        let kind = kinds
            .get(4 * i..4 * i + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        caps.push((kind, h));
    }
}

fn take_arguments() {
    let mut msg = Message::new();
    // The kernel queues it before the program starts: no waiting.
    if port_receive(bootstrap::MAIN_PORT, &mut msg, 0).is_err() || msg.tag != tag::BOOTSTRAP_ARGS {
        return;
    }
    let args = match msg.carried().first() {
        Some(&memory) => {
            let len = msg
                .payload()
                .get(..8)
                .map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])));
            let mut bytes = vec![0u8; len as usize];
            if let Ok(map) = Mapping::new(memory, len.next_multiple_of(4096), false) {
                let _ = map.read(0, &mut bytes);
            }
            let _ = handle_close(memory);
            split(&bytes)
        }
        None => split(msg.payload()),
    };
    *ARGS.lock() = args;
}

/// This program's arguments (the program name first).
pub fn args() -> Vec<String> {
    ARGS.lock().clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn splitting() {
        assert_eq!(super::split(b"ls\0-l\0x"), ["ls", "-l", "x"]);
        assert!(super::split(b"").is_empty());
        assert_eq!(super::split(b"one"), ["one"]);
    }
}
