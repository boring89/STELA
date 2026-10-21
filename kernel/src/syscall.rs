// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! System-call dispatch: decodes the number and arguments the program
//! left in its registers (`abi`), runs the call, and puts the status in
//! RAX and the value in RDX.

use crate::object::port::{Port, QueuedMessage};
use crate::object::tty::Tty;
use crate::object::waitset::WaitSet;
use crate::object::{Entry, KernelObject};
use crate::task::{self, Process};
use abi::layout::USER_END;
use abi::{
    Error, Handle, MEMORY_MAX, MESSAGE_DATA, MESSAGE_HANDLES, MESSAGE_SIZE, Message, Rights,
    Syscall, tag,
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use frame::mm::paging::MapError;
use frame::mm::shared::SharedMemory;
use frame::mm::user::UserFlags;
use frame::syscall::SyscallFrame;

/// Registers [`handle`] with frame. Call once at boot.
pub fn init() {
    frame::syscall::set_handler(handle);
}

/// The handler frame calls for every `syscall`. Linux processes' calls go
/// to `crate::linux`.
fn handle(frame: &mut SyscallFrame) {
    if let Some(process) = task::current_process()
        && process.personality() == task::Personality::Linux
    {
        crate::linux::dispatch(frame, process);
        return;
    }
    let args = frame.args();
    let [a0, a1, a2, ..] = args;
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
        Some(Syscall::HandleClose) => handle_close(a0),
        Some(Syscall::HandleDuplicate) => handle_duplicate(a0, a1),
        Some(Syscall::PortCreate) => port_create(),
        Some(Syscall::PortSend) => port_send(a0, a1),
        Some(Syscall::PortReceive) => port_receive(a0, a1, a2),
        Some(Syscall::MemoryCreate) => memory_create(a0),
        Some(Syscall::MemoryMap) => memory_map(a0, a1),
        Some(Syscall::MemoryUnmap) => memory_unmap(a0),
        Some(Syscall::InputAttach) => input_attach(a0),
        Some(Syscall::TtyCreate) => tty_create(a0),
        Some(Syscall::TtyRead) => tty_read(a0, a1, a2, args[3]),
        Some(Syscall::TtyWrite) => tty_write(a0, a1, a2),
        Some(Syscall::HandleKind) => handle_kind(a0),
        Some(Syscall::WaitSetCreate) => wait_set_create(),
        Some(Syscall::WaitSetAdd) => wait_set_add(a0, a1, a2, args[3]),
        Some(Syscall::WaitSetRemove) => wait_set_remove(a0, a1),
        Some(Syscall::WaitSetWait) => wait_set_wait(a0, a1, a2, args[3]),
        Some(call) => file_call(call, args),
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
    let target = {
        let handles = process.handles().lock();
        let entry = handles.get(Handle::from_raw(handle))?;
        let target = match &entry.object {
            KernelObject::Port(port) => SendTarget::Port(Arc::clone(port)),
            KernelObject::Tty(tty) => SendTarget::Tty(Arc::clone(tty)),
            _ => return Err(Error::WrongType),
        };
        if !entry.rights.contains(Rights::SEND) {
            return Err(Error::AccessDenied);
        }
        target
    };
    let mut bytes = [0u8; MESSAGE_SIZE];
    process
        .space()
        .read(msg_ptr, &mut bytes)
        .map_err(|_| Error::BadAddress)?;
    let mut msg = Message::from_bytes(&bytes);
    if msg.len as usize > MESSAGE_DATA || msg.handles as usize > MESSAGE_HANDLES {
        return Err(Error::InvalidArgument);
    }
    let port = match target {
        SendTarget::Port(port) => port,
        SendTarget::Tty(tty) => return tty_send(&tty, &msg),
    };
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

/// Where a `PortSend` goes.
enum SendTarget {
    Port(Arc<Port>),
    Tty(Arc<Tty>),
}

/// `PortSend` to a tty: `CONSOLE_WRITE` is output, `INPUT_KEY` is typed
/// (as the character it makes); other messages are ignored. Handles are
/// not taken (a tty has nowhere to keep them).
fn tty_send(tty: &Tty, msg: &Message) -> Result<u64, Error> {
    if msg.handles != 0 {
        return Err(Error::InvalidArgument);
    }
    match msg.tag {
        tag::CONSOLE_WRITE => tty.write(msg.payload()),
        tag::INPUT_KEY => {
            if let Some(c) =
                abi::key::KeyEvent::decode(msg.payload()).and_then(|e| abi::key::key_to_char(&e))
            {
                tty.feed(c);
            }
        }
        _ => {}
    }
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
    let object = {
        let handles = process.handles().lock();
        let entry = handles.get(Handle::from_raw(handle))?;
        let object = match &entry.object {
            KernelObject::Memory(memory) => Mappable::Memory(memory.clone()),
            KernelObject::Framebuffer(fb) => Mappable::Framebuffer(Arc::clone(fb)),
            _ => return Err(Error::WrongType),
        };
        let mut needed = Rights::MAP | Rights::READ;
        if writable {
            needed = needed | Rights::WRITE;
        }
        if !entry.rights.contains(needed) {
            return Err(Error::AccessDenied);
        }
        object
    };
    let size = match &object {
        Mappable::Memory(memory) => memory.size(),
        Mappable::Framebuffer(fb) => fb.range.size(),
    };
    let addr = process
        .maps()
        .lock()
        .alloc(size)
        .ok_or(Error::AlreadyMapped)?;
    let flags = UserFlags {
        writable,
        executable: false,
    };
    let mapped = match &object {
        Mappable::Memory(memory) => process.space().map_shared(addr, memory, flags),
        Mappable::Framebuffer(fb) => process.space().map_device(addr, &fb.range, writable, true),
    };
    if let Err(e) = mapped {
        process.maps().lock().free(addr);
        return Err(match e {
            MapError::OutOfMemory => Error::OutOfMemory,
            _ => Error::AlreadyMapped,
        });
    }
    if let Mappable::Framebuffer(_) = object {
        // The screen is the mapper's now (the window server): the kernel
        // console stops drawing, and only it may take the keyboard.
        frame::console::hand_over();
        crate::input::set_owner(process.id());
    }
    Ok(addr)
}

/// What `MemoryMap` can map.
enum Mappable {
    Memory(SharedMemory),
    Framebuffer(Arc<frame::console::FramebufferDevice>),
}

/// `MemoryUnmap(addr)`.
fn memory_unmap(addr: u64) -> Result<u64, Error> {
    let process = caller()?;
    match process.space().unmap_shared(addr) {
        Ok(memory) => drop(memory),
        Err(_) => {
            process
                .space()
                .unmap_device(addr)
                .map_err(|_| Error::InvalidArgument)?;
        }
    }
    process.maps().lock().free(addr);
    Ok(0)
}

/// `InputAttach(port)`.
fn input_attach(handle: u64) -> Result<u64, Error> {
    let process = caller()?;
    let target = {
        let handles = process.handles().lock();
        let entry = handles.get(Handle::from_raw(handle))?;
        if !entry.rights.contains(Rights::SEND) {
            return Err(Error::AccessDenied);
        }
        match &entry.object {
            KernelObject::Port(port) => crate::input::Target::Port(Arc::clone(port)),
            KernelObject::Tty(tty) => crate::input::Target::Tty(Arc::clone(tty)),
            _ => return Err(Error::WrongType),
        }
    };
    if !crate::input::may_attach(process.id()) {
        return Err(Error::AccessDenied);
    }
    crate::input::attach(target);
    Ok(0)
}

/// `TtyCreate(flags) -> slave | master << 32`.
fn tty_create(flags: u64) -> Result<u64, Error> {
    if flags & !abi::tty::MASTER != 0 {
        return Err(Error::InvalidArgument);
    }
    let process = caller()?;
    let master = flags & abi::tty::MASTER != 0;
    let tty = Tty::new(master);
    let mut handles = process.handles().lock();
    if handles.room() < 2 {
        return Err(Error::OutOfHandles);
    }
    let slave = handles.insert(Entry {
        object: KernelObject::Tty(Arc::clone(&tty)),
        rights: Rights::SEND | Rights::DUPLICATE | Rights::TRANSFER,
    })?;
    let master = if master {
        handles
            .insert(Entry {
                object: KernelObject::TtyMaster(tty),
                rights: Rights::READ | Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER,
            })?
            .raw()
    } else {
        0
    };
    Ok(slave.raw() | (master << 32))
}

/// The terminal behind a master handle with `rights`.
fn master_of(process: &Process, handle: u64, rights: Rights) -> Result<Arc<Tty>, Error> {
    let handles = process.handles().lock();
    let entry = handles.get(Handle::from_raw(handle))?;
    let KernelObject::TtyMaster(tty) = &entry.object else {
        return Err(Error::WrongType);
    };
    if !entry.rights.contains(rights) {
        return Err(Error::AccessDenied);
    }
    Ok(Arc::clone(tty))
}

/// Most bytes one `TtyRead` or `TtyWrite` moves.
const TTY_CHUNK: u64 = 4096;

/// `TtyRead(master, buf, len, timeout_ns) -> bytes`.
fn tty_read(handle: u64, buf: u64, len: u64, timeout: u64) -> Result<u64, Error> {
    let process = caller()?;
    let tty = master_of(&process, handle, Rights::READ)?;
    let len = len.min(TTY_CHUNK) as usize;
    if !process.space().check(buf, len, true) {
        return Err(Error::BadAddress);
    }
    let deadline = match timeout {
        u64::MAX => None,
        t => Some(frame::time::now_ns().saturating_add(t)),
    };
    let data = tty.master_read(len, deadline)?;
    process
        .space()
        .write(buf, &data)
        .map_err(|_| Error::BadAddress)?;
    Ok(data.len() as u64)
}

/// `TtyWrite(master, buf, len) -> bytes`: typed input.
fn tty_write(handle: u64, buf: u64, len: u64) -> Result<u64, Error> {
    let process = caller()?;
    let tty = master_of(&process, handle, Rights::WRITE)?;
    let len = len.min(TTY_CHUNK) as usize;
    let mut data = alloc::vec![0u8; len];
    process
        .space()
        .read(buf, &mut data)
        .map_err(|_| Error::BadAddress)?;
    for chunk in data.utf8_chunks() {
        tty.feed_str(chunk.valid());
    }
    Ok(len as u64)
}

/// `HandleKind(handle) -> kind`.
fn handle_kind(handle: u64) -> Result<u64, Error> {
    use abi::object_kind as k;
    let process = caller()?;
    let handles = process.handles().lock();
    Ok(match handles.get(Handle::from_raw(handle))?.object {
        KernelObject::Port(_) => k::PORT,
        KernelObject::Memory(_) => k::MEMORY,
        KernelObject::Directory(_) => k::DIRECTORY,
        KernelObject::File(_) => k::FILE,
        KernelObject::Process(_) => k::PROCESS,
        KernelObject::Tty(_) => k::TTY,
        KernelObject::TtyMaster(_) => k::TTY_MASTER,
        KernelObject::Framebuffer(_) => k::FRAMEBUFFER,
        KernelObject::Socket(_) => k::SOCKET,
        KernelObject::Audio(_) => k::AUDIO,
        KernelObject::WaitSet(_) => k::WAIT_SET,
    })
}

/// `WaitSetCreate() -> handle`.
fn wait_set_create() -> Result<u64, Error> {
    let process = caller()?;
    let entry = Entry {
        object: KernelObject::WaitSet(WaitSet::new()),
        rights: Rights::WAIT | Rights::DUPLICATE | Rights::TRANSFER,
    };
    Ok(process.handles().lock().insert(entry)?.raw())
}

/// The set behind `set` (needs `WAIT`) and the object behind `member`
/// with its rights.
fn set_and_member(
    process: &Process,
    set: u64,
    member: u64,
) -> Result<(Arc<WaitSet>, KernelObject, Rights), Error> {
    let handles = process.handles().lock();
    let entry = handles.get(Handle::from_raw(set))?;
    let KernelObject::WaitSet(ws) = &entry.object else {
        return Err(Error::WrongType);
    };
    if !entry.rights.contains(Rights::WAIT) {
        return Err(Error::AccessDenied);
    }
    let member = handles.get(Handle::from_raw(member))?;
    Ok((Arc::clone(ws), member.object.clone(), member.rights))
}

/// `WaitSetAdd(set, handle, key, mask)`.
fn wait_set_add(set: u64, member: u64, key: u64, mask: u64) -> Result<u64, Error> {
    let process = caller()?;
    let (ws, object, rights) = set_and_member(&process, set, member)?;
    let mask = u32::try_from(mask).map_err(|_| Error::InvalidArgument)?;
    // A set inside itself (or another set) is refused by `add`.
    ws.add(&object, rights, key, mask)?;
    Ok(0)
}

/// `WaitSetRemove(set, handle)`.
fn wait_set_remove(set: u64, member: u64) -> Result<u64, Error> {
    let process = caller()?;
    let (ws, object, _) = set_and_member(&process, set, member)?;
    ws.remove(&object)?;
    Ok(0)
}

/// `WaitSetWait(set, &mut [Ready], max, timeout_ns) -> count`.
fn wait_set_wait(set: u64, buf: u64, max: u64, timeout: u64) -> Result<u64, Error> {
    use abi::wait::{MAX_READY, READY_SIZE};
    let process = caller()?;
    let ws = {
        let handles = process.handles().lock();
        let entry = handles.get(Handle::from_raw(set))?;
        let KernelObject::WaitSet(ws) = &entry.object else {
            return Err(Error::WrongType);
        };
        if !entry.rights.contains(Rights::WAIT) {
            return Err(Error::AccessDenied);
        }
        Arc::clone(ws)
    };
    if max == 0 {
        return Err(Error::InvalidArgument);
    }
    let max = (max as usize).min(MAX_READY);
    if !process.space().check(buf, max * READY_SIZE, true) {
        return Err(Error::BadAddress);
    }
    let deadline = match timeout {
        u64::MAX => None,
        t => Some(frame::time::now_ns().saturating_add(t)),
    };
    let ready = ws.wait(deadline, max)?;
    let mut bytes = Vec::with_capacity(ready.len() * READY_SIZE);
    for r in &ready {
        bytes.extend_from_slice(&r.to_bytes());
    }
    process
        .space()
        .write(buf, &bytes)
        .map_err(|_| Error::BadAddress)?;
    Ok(ready.len() as u64)
}

/// The file system calls (`crate::fsyscall`).
fn file_call(call: Syscall, args: [u64; 6]) -> Result<u64, Error> {
    use crate::fsyscall::*;
    let p = caller()?;
    let [a0, a1, a2, a3, _, _] = args;
    match call {
        Syscall::DirOpen => dir_open(&p, a0, a1, a2, a3),
        Syscall::DirRead => dir_read(&p, a0, a1, a2),
        Syscall::DirRemove => dir_remove(&p, a0, a1, a2),
        Syscall::DirRename => dir_rename(&p, args),
        Syscall::FileRead => file_read(&p, a0, a1, a2, a3),
        Syscall::FileWrite => file_write(&p, a0, a1, a2, a3),
        Syscall::FileStat => file_stat(&p, a0, a1),
        Syscall::FileSetSize => file_set_size(&p, a0, a1),
        Syscall::VolumeList => volume_list(&p, a0, a1),
        Syscall::VolumeOpen => volume_open(&p, a0, a1),
        Syscall::VolumeOpenById => volume_open_by_id(&p, a0, a1),
        Syscall::ProcessSpawn => crate::psyscall::process_spawn(&p, a0, a1, a2, a3),
        Syscall::ProcessWait => crate::psyscall::process_wait(&p, a0, a1),
        Syscall::ProcessKill => crate::psyscall::process_kill(&p, a0),
        Syscall::ProcessSelf => crate::psyscall::process_self(&p),
        Syscall::SysInfo => crate::psyscall::sys_info(&p, a0, a1, a2),
        Syscall::SystemPower => crate::psyscall::system_power(a0),
        Syscall::FileGetInfo => file_get_info(&p, a0, a1),
        Syscall::FileSetInfo => file_set_info(&p, a0, a1),
        Syscall::SocketCreate => crate::net::socket_create(&p, a0),
        Syscall::SocketBind => crate::net::socket_bind(&p, a0, a1),
        Syscall::SocketConnect => crate::net::socket_connect(&p, a0, a1, a2),
        Syscall::SocketListen => crate::net::socket_listen(&p, a0, a1),
        Syscall::SocketAccept => crate::net::socket_accept(&p, a0, a1),
        Syscall::SocketSend => crate::net::socket_send(&p, a0, a1, a2, a3),
        Syscall::SocketRecv => crate::net::socket_recv(&p, args),
        Syscall::NetInfo => crate::net::net_info(&p, a0),
        Syscall::AudioOpen => crate::audio::audio_open(&p, a0, a1, a2),
        Syscall::AudioClose => crate::audio::audio_close(&p, a0),
        _ => Err(Error::InvalidSyscall),
    }
}
