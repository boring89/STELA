// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! System-call dispatch: decodes the number and arguments the program
//! left in its registers (`abi`), runs the call, and puts the status in
//! RAX and the value in RDX.

use crate::object::port::{Port, QueuedMessage};
use crate::object::{Entry, KernelObject};
use crate::task::{self, Process};
use abi::layout::USER_END;
use abi::{
    DEBUG_WRITE_MAX, Error, Handle, MEMORY_MAX, MESSAGE_DATA, MESSAGE_HANDLES, MESSAGE_SIZE,
    Message, Rights, Syscall,
};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use frame::mm::paging::MapError;
use frame::mm::shared::SharedMemory;
use frame::mm::user::UserFlags;
use frame::syscall::SyscallFrame;

/// Registers [`handle`] with frame. Call once at boot.
pub fn init() {
    frame::syscall::set_handler(handle);
}

/// The handler frame calls for every `syscall`.
fn handle(frame: &mut SyscallFrame) {
    let [a0, a1, a2, _a3, _a4, _a5] = frame.args();
    let result = match Syscall::from_raw(frame.number()) {
        Some(Syscall::ProcessExit) => task::exit_process(a0 as i64),
        Some(Syscall::ThreadExit) => task::exit(),
        Some(Syscall::ThreadCreate) => thread_create(a0, a1, a2),
        Some(Syscall::Yield) => {
            task::yield_now();
            Ok(0)
        }
        Some(Syscall::SleepNs) => {
            task::sleep_ns(a0);
            Ok(0)
        }
        Some(Syscall::ClockNowNs) => Ok(frame::time::now_ns()),
        Some(Syscall::DebugWrite) => debug_write(a0, a1),
        Some(Syscall::HandleClose) => handle_close(a0),
        Some(Syscall::HandleDuplicate) => handle_duplicate(a0, a1),
        Some(Syscall::PortCreate) => port_create(),
        Some(Syscall::PortSend) => port_send(a0, a1),
        Some(Syscall::PortReceive) => port_receive(a0, a1, a2),
        Some(Syscall::MemoryCreate) => memory_create(a0),
        Some(Syscall::MemoryMap) => memory_map(a0, a1),
        Some(Syscall::MemoryUnmap) => memory_unmap(a0),
        Some(Syscall::InputAttach) => Err(Error::NotSupported),
        None => Err(Error::InvalidSyscall),
    };
    let (status, value) = abi::encode(result);
    frame.set_return(status, value);
}

/// `ThreadCreate(entry, stack_top, arg)`.
fn thread_create(entry: u64, stack_top: u64, arg: u64) -> Result<u64, Error> {
    if entry >= USER_END || !(16..=USER_END).contains(&stack_top) {
        return Err(Error::InvalidArgument);
    }
    let Some(process) = task::current_process() else {
        return Err(Error::NotSupported);
    };
    // As if the thread had been called: RSP + 8 is 16-byte aligned.
    let rsp = (stack_top & !15) - 8;
    match task::spawn_user(&process, entry, rsp, arg) {
        Ok(id) => Ok(id),
        Err(task::SpawnError::OutOfMemory) => Err(Error::OutOfMemory),
        // The process is being torn down; this thread is about to go too.
        Err(task::SpawnError::Exiting) => Err(Error::NotSupported),
    }
}

/// `DebugWrite(ptr, len)`: copies the bytes from the caller's address
/// space (checking every page) and prints them.
fn debug_write(ptr: u64, len: u64) -> Result<u64, Error> {
    if len > DEBUG_WRITE_MAX {
        return Err(Error::InvalidArgument);
    }
    let Some(process) = task::current_process() else {
        return Err(Error::NotSupported);
    };
    let mut buf = vec![0u8; len as usize];
    process
        .space()
        .read(ptr, &mut buf)
        .map_err(|_| Error::BadAddress)?;
    for chunk in buf.utf8_chunks() {
        frame::kprint!("{}", chunk.valid());
        if !chunk.invalid().is_empty() {
            frame::kprint!("\u{FFFD}");
        }
    }
    Ok(len)
}

/// The calling process (system calls from kernel threads do not happen,
/// but are refused).
fn caller() -> Result<Arc<Process>, Error> {
    task::current_process().ok_or(Error::NotSupported)
}

/// The port behind `handle`, which needs `rights`.
fn port_of(process: &Process, handle: u64, rights: Rights) -> Result<Arc<Port>, Error> {
    let handles = process.handles().lock();
    let entry = handles.get(Handle::from_raw(handle))?;
    let port = entry.object.as_port().ok_or(Error::WrongType)?;
    if !entry.rights.contains(rights) {
        return Err(Error::AccessDenied);
    }
    Ok(Arc::clone(port))
}

/// `HandleClose(handle)`.
fn handle_close(handle: u64) -> Result<u64, Error> {
    let process = caller()?;
    let entry = process.handles().lock().remove(Handle::from_raw(handle))?;
    // The object may go with it; not under the table's lock.
    drop(entry);
    Ok(0)
}

/// `HandleDuplicate(handle, rights) -> handle`.
fn handle_duplicate(handle: u64, rights: u64) -> Result<u64, Error> {
    let rights = Rights::from_raw(rights).ok_or(Error::InvalidArgument)?;
    let process = caller()?;
    let new = process
        .handles()
        .lock()
        .duplicate(Handle::from_raw(handle), rights)?;
    Ok(new.raw())
}

/// `PortCreate() -> handle`.
fn port_create() -> Result<u64, Error> {
    let process = caller()?;
    let port = process.new_port();
    let entry = Entry {
        object: KernelObject::Port(port),
        rights: Rights::SEND | Rights::RECEIVE | Rights::DUPLICATE | Rights::TRANSFER,
    };
    let handle = process.handles().lock().insert(entry)?;
    Ok(handle.raw())
}

/// `PortSend(port, &Message)`: copies the message in, moves the carried
/// handles out of the caller's table and queues it.
fn port_send(handle: u64, msg_ptr: u64) -> Result<u64, Error> {
    let process = caller()?;
    let port = port_of(&process, handle, Rights::SEND)?;
    let mut bytes = [0u8; MESSAGE_SIZE];
    process
        .space()
        .read(msg_ptr, &mut bytes)
        .map_err(|_| Error::BadAddress)?;
    let mut msg = Message::from_bytes(&bytes);
    if msg.len as usize > MESSAGE_DATA || msg.handles as usize > MESSAGE_HANDLES {
        return Err(Error::InvalidArgument);
    }
    // Refuse early when the message could not be queued, so that the
    // handles stay where they are. (A send racing with this one may still
    // fill the queue; the handles are then released with the message.)
    port.check_send()?;
    let carried: Vec<Handle> = msg.carried().to_vec();
    let objects = {
        let mut handles = process.handles().lock();
        for (i, h) in carried.iter().enumerate() {
            if carried[..i].contains(h) {
                return Err(Error::InvalidArgument);
            }
            handles.get_with(*h, Rights::TRANSFER)?;
        }
        let mut objects = Vec::with_capacity(carried.len());
        for h in &carried {
            objects.push(handles.remove(*h)?);
        }
        objects
    };
    msg.handle_slots = [Handle::NONE; MESSAGE_HANDLES];
    msg.reserved = 0;
    port.send(QueuedMessage { msg, objects })?;
    Ok(0)
}

/// `PortReceive(port, &mut Message, timeout_ns)`: takes a message (waiting
/// up to the timeout), puts the handles it carries in the caller's table
/// and copies it out.
fn port_receive(handle: u64, msg_ptr: u64, timeout: u64) -> Result<u64, Error> {
    let process = caller()?;
    let port = port_of(&process, handle, Rights::RECEIVE)?;
    if !process.space().check(msg_ptr, MESSAGE_SIZE, true) {
        return Err(Error::BadAddress);
    }
    let deadline = match timeout {
        u64::MAX => None,
        t => Some(frame::time::now_ns().saturating_add(t)),
    };
    let room = process.handles().lock().room().min(MESSAGE_HANDLES);
    let QueuedMessage { mut msg, objects } = port.receive(deadline, room)?;
    {
        let mut handles = process.handles().lock();
        msg.handles = 0;
        for entry in objects {
            // Room was checked; only another thread of this process taking
            // the slots meanwhile makes this fail, and then the object is
            // released.
            let h = handles.insert(entry)?;
            msg.handle_slots[msg.handles as usize] = h;
            msg.handles += 1;
        }
    }
    process
        .space()
        .write(msg_ptr, &msg.to_bytes())
        .map_err(|_| Error::BadAddress)?;
    Ok(0)
}

/// `MemoryCreate(size) -> handle`.
fn memory_create(size: u64) -> Result<u64, Error> {
    if size == 0 || !size.is_multiple_of(4096) || size > MEMORY_MAX {
        return Err(Error::InvalidArgument);
    }
    let process = caller()?;
    let memory = SharedMemory::new((size / 4096) as usize).ok_or(Error::OutOfMemory)?;
    let entry = Entry {
        object: KernelObject::Memory(memory),
        rights: Rights::READ | Rights::WRITE | Rights::MAP | Rights::DUPLICATE | Rights::TRANSFER,
    };
    let handle = process.handles().lock().insert(entry)?;
    Ok(handle.raw())
}

/// `MemoryMap(memory, flags) -> address`.
fn memory_map(handle: u64, flags: u64) -> Result<u64, Error> {
    if flags & !abi::map::ALL != 0 {
        return Err(Error::InvalidArgument);
    }
    let writable = flags & abi::map::WRITE != 0;
    let process = caller()?;
    let memory = {
        let handles = process.handles().lock();
        let entry = handles.get(Handle::from_raw(handle))?;
        let memory = entry.object.as_memory().ok_or(Error::WrongType)?;
        let mut needed = Rights::MAP | Rights::READ;
        if writable {
            needed = needed | Rights::WRITE;
        }
        if !entry.rights.contains(needed) {
            return Err(Error::AccessDenied);
        }
        memory.clone()
    };
    let addr = process
        .maps()
        .lock()
        .alloc(memory.size())
        .ok_or(Error::AlreadyMapped)?;
    let flags = UserFlags {
        writable,
        executable: false,
    };
    if let Err(e) = process.space().map_shared(addr, &memory, flags) {
        process.maps().lock().free(addr);
        return Err(match e {
            MapError::OutOfMemory => Error::OutOfMemory,
            _ => Error::AlreadyMapped,
        });
    }
    Ok(addr)
}

/// `MemoryUnmap(address)`.
fn memory_unmap(addr: u64) -> Result<u64, Error> {
    let process = caller()?;
    let memory = process
        .space()
        .unmap_shared(addr)
        .map_err(|_| Error::InvalidArgument)?;
    process.maps().lock().free(addr);
    drop(memory);
    Ok(0)
}
