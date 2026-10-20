// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Processes (`docs/userland.md`): starting a program from a file handle
//! with arguments and handles, waiting, killing, and the arguments this
//! program got.

use crate::ipc::{FOREVER, Mapping, handle_close, memory_create, port_receive};
use crate::sys::{syscall0, syscall1, syscall2, syscall3, syscall4};
use abi::sysinfo::{self, MemInfo, PROCESS_INFO_SIZE, ProcessInfo};
use abi::{Error, Handle, MESSAGE_DATA, Message, Rights, Syscall, bootstrap, tag};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Starts the program in `file` (a handle with `READ`) with `args` (the
/// program name first) and `handles` as its `[1]`, `[2]`, `[3]`
/// (`Handle::NONE` for none; each is moved and needs `TRANSFER`).
/// Returns a handle with `WAIT | KILL`.
pub fn spawn(file: Handle, args: &[&str], handles: [Handle; 3]) -> Result<Handle, Error> {
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
    let raw: [u8; 12] = {
        let mut r = [0u8; 12];
        for (i, h) in handles.iter().enumerate() {
            r[4 * i..4 * i + 4].copy_from_slice(&h.0.to_le_bytes());
        }
        r
    };
    syscall4(
        Syscall::ProcessSpawn,
        file.raw(),
        &msg as *const Message as u64,
        raw.as_ptr() as u64,
        3,
    )
    .map(Handle::from_raw)
}

/// A `TRANSFER`-able duplicate of `h` with its own rights (to pass to a
/// child), or `Handle::NONE` if `h` is not valid.
pub fn pass(h: Handle, rights: Rights) -> Handle {
    crate::ipc::handle_duplicate(h, rights.union(Rights::TRANSFER)).unwrap_or(Handle::NONE)
}

/// The standard handles for a child: the console and `cwd` (both
/// duplicated), and `bootstrap` as `[2]`.
pub fn standard_handles(bootstrap_handle: Handle, cwd: Handle) -> [Handle; 3] {
    [
        pass(bootstrap::CONSOLE, Rights::SEND | Rights::DUPLICATE),
        bootstrap_handle,
        pass(cwd, Rights::READ | Rights::WRITE | Rights::DUPLICATE),
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

/// Whether the boot options set `key` (`sysinfo::cmdline_flag`).
pub fn boot_flag(key: &str) -> bool {
    sysinfo::cmdline_flag(&cmdline(), key)
}

/// Flushes the volumes and turns the machine off (`SystemPower`). Returns
/// only on failure (`NotSupported` when the machine has no ACPI soft off).
pub fn shutdown() -> Error {
    match syscall1(Syscall::SystemPower, abi::power::SHUTDOWN) {
        Ok(_) => Error::NotSupported,
        Err(e) => e,
    }
}

/// Flushes the volumes and restarts the machine (`SystemPower`). Returns
/// only on failure.
pub fn reboot() -> Error {
    match syscall1(Syscall::SystemPower, abi::power::REBOOT) {
        Ok(_) => Error::NotSupported,
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

/// Takes the `BOOTSTRAP_ARGS` message the kernel queued first on the main
/// port. Called by `entry!` before `main`.
#[doc(hidden)]
pub fn take_args() {
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
