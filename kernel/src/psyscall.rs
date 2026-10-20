// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The process system calls (`docs/userland.md`): starting a program from
//! a file handle, waiting for and killing processes through handles, and
//! system information.

use crate::object::port::QueuedMessage;
use crate::object::{Entry, KernelObject};
use crate::program::{self, LoadError};
use crate::task::{self, Process, ProcessId, process::WaitError};
use abi::sysinfo::{MemInfo, ProcessInfo};
use abi::{Error, Handle, MESSAGE_DATA, MESSAGE_HANDLES, MESSAGE_SIZE, Message, Rights, tag};
use alloc::vec::Vec;

/// Most initial handles `ProcessSpawn` takes (`[1]`..`[3]`).
const INITIAL: u64 = 3;

fn load_error(e: LoadError) -> Error {
    match e {
        LoadError::NotFound => Error::NotFound,
        LoadError::BadElf(_) | LoadError::BadSegment => Error::InvalidArgument,
        LoadError::OutOfMemory => Error::OutOfMemory,
    }
}

/// `ProcessSpawn(file, &Message, &[Handle; count], count) -> handle`.
pub fn process_spawn(
    process: &Process,
    file: u64,
    msg_ptr: u64,
    handles_ptr: u64,
    count: u64,
) -> Result<u64, Error> {
    let flags = count & !0xFFFF_FFFF;
    let count = count & 0xFFFF_FFFF;
    if count > INITIAL || flags & !abi::spawn::ALL != 0 || flags == abi::spawn::ALL {
        return Err(Error::InvalidArgument);
    }
    let force = if flags & abi::spawn::FORCE_LINUX != 0 {
        Some(task::Personality::Linux)
    } else if flags & abi::spawn::FORCE_STELA != 0 {
        Some(task::Personality::Stela)
    } else {
        None
    };
    let (node, bundle) = {
        let handles = process.handles().lock();
        let entry = handles.get_with(Handle::from_raw(file), Rights::READ)?;
        match &entry.object {
            KernelObject::File(node) => (node.clone(), None),
            KernelObject::Directory(dir) => (dir.clone(), Some(())),
            _ => return Err(Error::WrongType),
        }
    };
    // A bundle (`Name.app/`) runs its `exec` (`docs/mouseion.md` §5).
    let (node, bundle_name) = match bundle {
        None => (node, None),
        Some(()) => {
            let info = node.volume.fs.get(node.id)?;
            let Some(name) = program::bundle_name(&info.name) else {
                return Err(Error::IsDir);
            };
            let exec = node.volume.fs.lookup(node.id, program::BUNDLE_EXEC)?;
            let name = alloc::string::String::from(name);
            (
                crate::vfs::Node {
                    volume: node.volume,
                    id: exec.id,
                },
                Some(name),
            )
        }
    };
    let (file_name, image) = program::read_program(&node.volume, node.id).map_err(load_error)?;
    program::check_image(&image).map_err(|_| Error::InvalidArgument)?;
    // The initial handles and the message's handles: checked, then moved.
    let mut raw = [0u8; 12];
    process
        .space()
        .read(handles_ptr, &mut raw[..count as usize * 4])
        .map_err(|_| Error::BadAddress)?;
    let initial_handles: Vec<Handle> = raw[..count as usize * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| Handle(u32::from_le_bytes(*c)))
        .collect();
    let mut msg = if msg_ptr == 0 {
        Message::with_data(
            tag::BOOTSTRAP_ARGS,
            bundle_name.as_deref().unwrap_or(&file_name).as_bytes(),
        )
    } else {
        let mut b = [0u8; MESSAGE_SIZE];
        process
            .space()
            .read(msg_ptr, &mut b)
            .map_err(|_| Error::BadAddress)?;
        Message::from_bytes(&b)
    };
    if msg.len as usize > MESSAGE_DATA || msg.handles as usize > MESSAGE_HANDLES {
        return Err(Error::InvalidArgument);
    }
    msg.tag = tag::BOOTSTRAP_ARGS;
    // The process is called by its first argument (the program's name as
    // the caller sees it), else by the bundle's or the file's name.
    let name = match msg.payload().split(|&b| b == 0).next() {
        Some(first) if msg.handles == 0 && !first.is_empty() && first.len() <= 24 => {
            alloc::string::String::from_utf8_lossy(first).into_owned()
        }
        _ => bundle_name.unwrap_or(file_name),
    };
    let carried: Vec<Handle> = msg.carried().to_vec();
    let mut all: Vec<Handle> = initial_handles
        .iter()
        .copied()
        .filter(|h| *h != Handle::NONE)
        .collect();
    all.extend(carried.iter().copied());
    let (initial, objects) = {
        let mut handles = process.handles().lock();
        for (i, h) in all.iter().enumerate() {
            if all[..i].contains(h) {
                return Err(Error::InvalidArgument);
            }
            handles.get_with(*h, Rights::TRANSFER)?;
        }
        let mut initial: task::process::Initial = [None, None, None];
        for (slot, h) in initial.iter_mut().zip(initial_handles.iter()) {
            if *h != Handle::NONE {
                *slot = Some(handles.remove(*h)?);
            }
        }
        let mut objects: Vec<Entry> = Vec::with_capacity(carried.len());
        for h in &carried {
            objects.push(handles.remove(*h)?);
        }
        (initial, objects)
    };
    msg.handle_slots = [Handle::NONE; MESSAGE_HANDLES];
    msg.reserved = 0;
    let child = program::spawn_process(
        &name,
        &image,
        initial,
        Some(QueuedMessage { msg, objects }),
        force,
    )
    .map_err(load_error)?;
    let entry = Entry {
        object: KernelObject::Process(child.id()),
        rights: Rights::WAIT | Rights::KILL | Rights::DUPLICATE | Rights::TRANSFER,
    };
    let h = process.handles().lock().insert(entry)?;
    Ok(h.raw())
}

/// The process id behind `handle`, which needs `rights`.
fn pid_of(process: &Process, handle: u64, rights: Rights) -> Result<ProcessId, Error> {
    let handles = process.handles().lock();
    let entry = handles.get(Handle::from_raw(handle))?;
    let KernelObject::Process(pid) = entry.object else {
        return Err(Error::WrongType);
    };
    if !entry.rights.contains(rights) {
        return Err(Error::AccessDenied);
    }
    Ok(pid)
}

/// `ProcessWait(process, timeout_ns) -> exit code`.
pub fn process_wait(process: &Process, handle: u64, timeout: u64) -> Result<u64, Error> {
    let pid = pid_of(process, handle, Rights::WAIT)?;
    let deadline = match timeout {
        u64::MAX => None,
        t => Some(frame::time::now_ns().saturating_add(t)),
    };
    match task::process::wait_until(pid, deadline) {
        Ok(code) => Ok(code as u64),
        Err(WaitError::TimedOut) => Err(Error::TimedOut),
        Err(WaitError::Unknown) => Err(Error::NotFound),
        Err(WaitError::Killed) => Err(Error::NotSupported),
    }
}

/// `ProcessKill(process)`.
pub fn process_kill(process: &Process, handle: u64) -> Result<u64, Error> {
    let pid = pid_of(process, handle, Rights::KILL)?;
    if pid == process.id() {
        task::exit_process(abi::EXIT_KILLED);
    }
    if let Some(target) = task::process::find(pid) {
        task::kill(&target, abi::EXIT_KILLED);
    }
    Ok(0)
}

/// `ProcessSelf() -> handle`.
pub fn process_self(process: &Process) -> Result<u64, Error> {
    let entry = Entry {
        object: KernelObject::Process(process.id()),
        rights: Rights::KILL | Rights::DUPLICATE | Rights::TRANSFER,
    };
    let h = process.handles().lock().insert(entry)?;
    Ok(h.raw())
}

/// `SysInfo(kind, buf, len) -> count`.
pub fn sys_info(process: &Process, kind: u64, ptr: u64, len: u64) -> Result<u64, Error> {
    let mut out: Vec<u8> = Vec::new();
    let count = match kind {
        abi::sysinfo::PROCESSES => {
            let list = task::process::list();
            for (pid, name, threads, code) in &list {
                let mut n = [0u8; 24];
                let bytes = name.as_bytes();
                let k = bytes.len().min(24);
                n[..k].copy_from_slice(&bytes[..k]);
                let info = ProcessInfo {
                    pid: *pid,
                    threads: *threads as u32,
                    state: if code.is_some() { 2 } else { 1 },
                    exit_code: code.unwrap_or(0),
                    name: n,
                };
                out.extend_from_slice(&info.to_bytes());
            }
            list.len() as u64
        }
        abi::sysinfo::MEMORY => {
            let (free, total) = frame::mm::pmm::stats();
            let heap = frame::mm::heap::stats();
            let info = MemInfo {
                total_frames: total as u64,
                free_frames: free as u64,
                heap_size: heap.size as u64,
                heap_used: heap.used as u64,
            };
            out.extend_from_slice(&info.to_bytes());
            1
        }
        abi::sysinfo::UPTIME => {
            out.extend_from_slice(&frame::time::now_ns().to_le_bytes());
            out.extend_from_slice(&frame::time::ticks().to_le_bytes());
            1
        }
        abi::sysinfo::FRAMEBUFFER => match frame::console::framebuffer() {
            Some(fb) => {
                let info = abi::sysinfo::FramebufferInfo {
                    width: fb.width,
                    height: fb.height,
                    pitch: fb.pitch,
                    format: if fb.bgr {
                        abi::sysinfo::FORMAT_BGRX
                    } else {
                        abi::sysinfo::FORMAT_RGBX
                    },
                    size: fb.range.size() as u32,
                    scale: 1,
                };
                out.extend_from_slice(&info.to_bytes());
                1
            }
            None => 0,
        },
        abi::sysinfo::CPUS => {
            let n = frame::cpu::online();
            for id in 0..n {
                out.extend_from_slice(&kernel_ticks_of(id).to_le_bytes());
            }
            n as u64
        }
        abi::sysinfo::PROCESS_CPUS => {
            let list = task::process::cpus();
            for (pid, cpu) in &list {
                out.extend_from_slice(&pid.to_le_bytes());
                out.extend_from_slice(&(*cpu as u64).to_le_bytes());
            }
            list.len() as u64
        }
        abi::sysinfo::TIME => {
            out.extend_from_slice(&frame::time::wall_clock_ns().to_le_bytes());
            1
        }
        abi::sysinfo::CMDLINE => {
            let cmdline = frame::cmdline();
            let n = cmdline.len().min(abi::sysinfo::CMDLINE_SIZE);
            out.extend_from_slice(&cmdline[..n]);
            n as u64
        }
        _ => return Err(Error::InvalidArgument),
    };
    let n = out.len().min(len as usize);
    process
        .space()
        .write(ptr, &out[..n])
        .map_err(|_| Error::BadAddress)?;
    Ok(count)
}

/// Timer ticks processor `id` has handled.
fn kernel_ticks_of(id: usize) -> u64 {
    frame::cpu::ticks_of(id)
}

/// `SystemPower(kind)`: flushes and unmounts the volumes, then turns the
/// machine off or restarts it. Any program may call it for now (no
/// right guards it yet; `docs/acpi.md`).
pub fn system_power(kind: u64) -> Result<u64, Error> {
    match kind {
        abi::power::SHUTDOWN => match crate::power::shutdown() {
            frame::power::PowerError::NotSupported => Err(Error::NotSupported),
            frame::power::PowerError::Failed => Err(Error::IoError),
        },
        abi::power::REBOOT => crate::power::reboot(),
        _ => Err(Error::InvalidArgument),
    }
}
