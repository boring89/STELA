// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Networking (`docs/net.md`): the `net` crate's stack on the first
//! network card, a kernel thread (`net`) that feeds it, and the socket
//! system calls.
//!
//! ```text
//!  card interrupt (or a 5 ms poll without MSI) ─► net thread:
//!      every received frame ─► stack.receive; stack.poll (timers);
//!      sockets closed by their last handle; frames out to the card;
//!      wake the threads waiting in socket calls when something happened
//!  socket call ─► lock, stack operation, frames out; if it must wait:
//!      join the waiters, unlock, sleep (until woken or the deadline)
//! ```
//!
//! The stack lives behind one SpinLock (its operations are short and
//! never sleep); a waiting call re-checks its condition after every
//! wake-up.

use crate::object::{Entry, KernelObject};
use crate::task::queue::WaitQueue;
use crate::task::{self, Process};
use abi::{Error, Handle, Rights};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use drivers::net::NetDevice;
use frame::sync::SpinLock;
use net::{Kind, NetError, SocketId, Stack};

struct Net {
    stack: Stack,
    card: Arc<dyn NetDevice>,
    waiters: WaitQueue,
}

static NET: SpinLock<Option<Net>> = SpinLock::new(None);
/// Sockets whose last handle went away: closed by the net thread (a
/// handle may drop while NET is held).
static CLOSING: SpinLock<Vec<SocketId>> = SpinLock::new(Vec::new());
/// The net thread, for wake-ups (0 before it runs).
static THREAD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A socket: what a `Socket` handle refers to.
pub struct Socket {
    id: SocketId,
}

impl Drop for Socket {
    fn drop(&mut self) {
        CLOSING.lock().push(self.id);
        task::wake(THREAD.load(core::sync::atomic::Ordering::Acquire));
    }
}

fn now_ms() -> u64 {
    frame::time::now_ns() / 1_000_000
}

/// Sends the frames the stack queued and wakes waiters if something
/// happened.
fn flush(net: &mut Net) {
    for f in net.stack.take_output() {
        let _ = net.card.send(&f);
    }
    if net.stack.take_events() {
        while let Some(id) = net.waiters.pop() {
            task::wake(id);
        }
    }
}

/// Probes the network card and starts the net thread with DHCP. Call
/// once at boot, after `block::probe`. The boot option `nonet` leaves the
/// card alone.
pub fn start() {
    if frame::boot_flag("nonet") {
        frame::kprintln!("net: off (boot option nonet)");
        return;
    }
    let Some(card) = drivers::net::probe() else {
        return;
    };
    let mac = card.mac();
    let seed = frame::time::now_ns() ^ frame::time::wall_clock_ns().rotate_left(29);
    *NET.lock() = Some(Net {
        stack: Stack::new(mac, seed),
        card,
        waiters: WaitQueue::new(),
    });
    if let Err(e) = task::spawn("net", run) {
        frame::kprintln!("net: cannot start the thread: {e:?}");
    }
}

fn run() {
    let me = task::current_id();
    THREAD.store(me, core::sync::atomic::Ordering::Release);
    drivers::net::set_waiter(me);
    let interrupts = {
        let mut guard = NET.lock();
        let Some(net) = guard.as_mut() else { return };
        net.stack.start_dhcp(now_ms());
        flush(net);
        net.card.interrupts()
    };
    let mut buf = vec![0u8; drivers::net::BUFFER_SIZE];
    let mut reported = false;
    loop {
        let closing: Vec<SocketId> = core::mem::take(&mut *CLOSING.lock());
        {
            let mut guard = NET.lock();
            let Some(net) = guard.as_mut() else { return };
            let now = now_ms();
            for id in closing {
                net.stack.close(id, now);
            }
            while let Some(n) = net.card.receive(&mut buf) {
                net.stack.receive(&buf[..n], now);
            }
            net.stack.poll(now);
            flush(net);
            if !reported && net.stack.configured() {
                reported = true;
                let c = net.stack.config();
                let [a, b, cc, d] = c.ip;
                let [g0, g1, g2, g3] = c.gateway;
                let [d0, d1, d2, d3] = c.dns;
                frame::kprintln!(
                    "net: DHCP: {a}.{b}.{cc}.{d}, gateway {g0}.{g1}.{g2}.{g3}, DNS {d0}.{d1}.{d2}.{d3}"
                );
            }
        }
        // With MSI the card wakes us; the timers need 100 ms steps.
        let step = if interrupts { 100 } else { 5 };
        task::block_until(Some(frame::time::now_ns() + step * 1_000_000));
    }
}

fn net_error(e: NetError) -> Error {
    match e {
        NetError::BadSocket => Error::BadHandle,
        NetError::InvalidState => Error::InvalidArgument,
        NetError::PortInUse => Error::AddressInUse,
        NetError::Unconfigured => Error::NetworkDown,
        NetError::Refused => Error::ConnectionRefused,
        NetError::Reset => Error::ConnectionReset,
        NetError::TimedOut => Error::TimedOut,
        NetError::NoResources => Error::OutOfHandles,
        NetError::TooLarge => Error::InvalidArgument,
    }
}

/// Runs `f` on the stack; `NetworkDown` without a card.
fn with<T>(f: impl FnOnce(&mut Net) -> Result<T, Error>) -> Result<T, Error> {
    let mut guard = NET.lock();
    let net = guard.as_mut().ok_or(Error::NetworkDown)?;
    let r = f(net);
    flush(net);
    r
}

/// Calls `try_now` until it gives an answer (`Some`), sleeping between
/// tries until something happens in the stack; `TimedOut` after
/// `deadline` (`None`: forever).
fn wait<T>(
    deadline: Option<u64>,
    mut try_now: impl FnMut(&mut Net) -> Option<Result<T, Error>>,
) -> Result<T, Error> {
    let me = task::current_id();
    loop {
        if task::current_killed() {
            return Err(Error::TimedOut);
        }
        {
            let mut guard = NET.lock();
            let net = guard.as_mut().ok_or(Error::NetworkDown)?;
            if let Some(r) = try_now(net) {
                net.waiters.remove(me);
                flush(net);
                return r;
            }
            flush(net);
            if deadline.is_some_and(|d| frame::time::now_ns() >= d) {
                net.waiters.remove(me);
                return Err(Error::TimedOut);
            }
            net.waiters.add(me);
        }
        task::block_until(deadline);
    }
}

/// The `abi::wait` events of socket `s` (`CLOSED` without a card).
pub fn events(s: &Socket) -> u32 {
    use abi::wait;
    let mut guard = NET.lock();
    let Some(net) = guard.as_mut() else {
        return wait::CLOSED;
    };
    let r = net.stack.readiness(s.id);
    let mut e = 0;
    if r.readable {
        e |= wait::READABLE;
    }
    if r.writable {
        e |= wait::WRITABLE;
    }
    if r.connected {
        e |= wait::CONNECTED;
    }
    if r.closed {
        e |= wait::CLOSED;
    }
    e
}

/// Thread `me` (in a `WaitSet` wait) is woken by the next change in the
/// stack.
pub fn watch(me: task::ThreadId) {
    if let Some(net) = NET.lock().as_mut() {
        net.waiters.add(me);
    }
}

/// Undoes [`watch`].
pub fn unwatch(me: task::ThreadId) {
    if let Some(net) = NET.lock().as_mut() {
        net.waiters.remove(me);
    }
}

fn deadline(timeout: u64) -> Option<u64> {
    match timeout {
        u64::MAX => None,
        t => Some(frame::time::now_ns().saturating_add(t)),
    }
}

/// A new socket of `kind`.
pub fn create(kind: Kind) -> Result<Arc<Socket>, Error> {
    let id = with(|net| net.stack.socket(kind).map_err(net_error))?;
    Ok(Arc::new(Socket { id }))
}

/// Binds local port `port` (0: any); returns the port.
pub fn bind(s: &Socket, port: u16) -> Result<u16, Error> {
    with(|net| net.stack.bind(s.id, port).map_err(net_error))
}

/// TCP: connects, waiting up to `timeout` ns (`u64::MAX`: forever);
/// UDP: sets the default destination.
pub fn connect(s: &Socket, ip: [u8; 4], port: u16, timeout: u64) -> Result<(), Error> {
    with(|net| {
        net.stack
            .connect(s.id, ip, port, now_ms())
            .map_err(net_error)
    })?;
    let udp = with(|net| Ok(net.stack.connected(s.id) == Err(NetError::InvalidState)))?;
    if udp {
        return Ok(());
    }
    let r = wait(deadline(timeout), |net| match net.stack.connected(s.id) {
        Ok(true) => Some(Ok(())),
        Ok(false) => None,
        Err(e) => Some(Err(net_error(e))),
    });
    if r == Err(Error::TimedOut) {
        // Give up the half-open connection.
        let _ = with(|net| {
            net.stack.close(s.id, now_ms());
            Ok(())
        });
    }
    r
}

/// TCP: listens on the bound port.
pub fn listen(s: &Socket, backlog: usize) -> Result<(), Error> {
    with(|net| net.stack.listen(s.id, backlog).map_err(net_error))
}

/// TCP: the next connection, waiting up to `timeout` ns.
pub fn accept(s: &Socket, timeout: u64) -> Result<Arc<Socket>, Error> {
    let id = wait(deadline(timeout), |net| match net.stack.accept(s.id) {
        Ok(Some(id)) => Some(Ok(id)),
        Ok(None) => None,
        Err(e) => Some(Err(net_error(e))),
    })?;
    Ok(Arc::new(Socket { id }))
}

/// Sends `data` (TCP: waits for room and returns how much was queued;
/// UDP: one datagram to `to` or the connected peer; ICMP: an echo request
/// with `to`'s port as the sequence number).
pub fn send(s: &Socket, data: &[u8], to: Option<([u8; 4], u16)>) -> Result<usize, Error> {
    wait(None, |net| match net.stack.send(s.id, data, to, now_ms()) {
        // TCP with a full buffer: wait for room.
        Ok(0) if !data.is_empty() => None,
        Ok(n) => Some(Ok(n)),
        Err(e) => Some(Err(net_error(e))),
    })
}

/// Receives up to `max` bytes, waiting up to `timeout` ns: the data
/// (TCP: empty at the end of the stream) and its source.
pub fn recv(s: &Socket, max: usize, timeout: u64) -> Result<(Vec<u8>, [u8; 4], u16), Error> {
    let mut data = vec![0u8; max];
    let (n, ip, port) = wait(deadline(timeout), |net| {
        match net.stack.recv(s.id, &mut data) {
            Ok(Some(r)) => Some(Ok(r)),
            Ok(None) => None,
            Err(e) => Some(Err(net_error(e))),
        }
    })?;
    data.truncate(n);
    Ok((data, ip, port))
}

/// TCP: no more sending (FIN), reading goes on.
pub fn shutdown(s: &Socket) -> Result<(), Error> {
    with(|net| net.stack.shutdown(s.id, now_ms()).map_err(net_error))
}

/// Our address and the socket's local port.
pub fn local_addr(s: &Socket) -> Result<([u8; 4], u16), Error> {
    with(|net| {
        let port = net.stack.local_port(s.id).map_err(net_error)?;
        Ok((net.stack.config().ip, port))
    })
}

/// The connected peer.
pub fn peer_addr(s: &Socket) -> Result<([u8; 4], u16), Error> {
    with(|net| net.stack.peer(s.id).ok_or(Error::InvalidArgument))
}

/// The socket behind `handle`.
fn socket_of(process: &Process, handle: u64) -> Result<Arc<Socket>, Error> {
    let handles = process.handles().lock();
    let entry = handles.get(Handle::from_raw(handle))?;
    match &entry.object {
        KernelObject::Socket(s) => Ok(Arc::clone(s)),
        _ => Err(Error::WrongType),
    }
}

fn insert_socket(process: &Process, socket: Arc<Socket>) -> Result<u64, Error> {
    let entry = Entry {
        object: KernelObject::Socket(socket),
        rights: Rights::READ | Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER,
    };
    let h = process.handles().lock().insert(entry)?;
    Ok(h.raw())
}

/// `SocketCreate(kind) -> handle`.
pub fn socket_create(process: &Process, kind: u64) -> Result<u64, Error> {
    let kind = match kind {
        abi::net::UDP => Kind::Udp,
        abi::net::TCP => Kind::Tcp,
        abi::net::ICMP => Kind::Icmp,
        _ => return Err(Error::InvalidArgument),
    };
    insert_socket(process, create(kind)?)
}

/// `SocketBind(socket, port) -> port`.
pub fn socket_bind(process: &Process, handle: u64, port: u64) -> Result<u64, Error> {
    let port = u16::try_from(port).map_err(|_| Error::InvalidArgument)?;
    bind(&*socket_of(process, handle)?, port).map(u64::from)
}

/// `SocketConnect(socket, addr, timeout)`.
pub fn socket_connect(
    process: &Process,
    handle: u64,
    addr: u64,
    timeout: u64,
) -> Result<u64, Error> {
    let (ip, port) = abi::net::unpack(addr);
    connect(&*socket_of(process, handle)?, ip, port, timeout).map(|()| 0)
}

/// `SocketListen(socket, backlog)`.
pub fn socket_listen(process: &Process, handle: u64, backlog: u64) -> Result<u64, Error> {
    listen(&*socket_of(process, handle)?, backlog as usize).map(|()| 0)
}

/// `SocketAccept(socket, timeout) -> handle`.
pub fn socket_accept(process: &Process, handle: u64, timeout: u64) -> Result<u64, Error> {
    let conn = accept(&*socket_of(process, handle)?, timeout)?;
    insert_socket(process, conn)
}

/// Most bytes one send or receive moves.
pub const CHUNK: usize = 64 * 1024;

/// `SocketSend(socket, buf, len, addr) -> bytes`.
pub fn socket_send(
    process: &Process,
    handle: u64,
    buf: u64,
    len: u64,
    addr: u64,
) -> Result<u64, Error> {
    let s = socket_of(process, handle)?;
    let len = (len as usize).min(CHUNK);
    let mut data = vec![0u8; len];
    process
        .space()
        .read(buf, &mut data)
        .map_err(|_| Error::BadAddress)?;
    let to = (addr != 0).then(|| abi::net::unpack(addr));
    send(&s, &data, to).map(|n| n as u64)
}

/// `SocketRecv(socket, buf, len, timeout, &mut addr) -> bytes`.
pub fn socket_recv(process: &Process, args: [u64; 6]) -> Result<u64, Error> {
    let [handle, buf, len, timeout, addr_out, _] = args;
    let s = socket_of(process, handle)?;
    let len = (len as usize).min(CHUNK);
    if !process.space().check(buf, len, true) {
        return Err(Error::BadAddress);
    }
    let (data, ip, port) = recv(&s, len, timeout)?;
    process
        .space()
        .write(buf, &data)
        .map_err(|_| Error::BadAddress)?;
    if addr_out != 0 {
        process
            .space()
            .write(addr_out, &abi::net::pack(ip, port).to_le_bytes())
            .map_err(|_| Error::BadAddress)?;
    }
    Ok(data.len() as u64)
}

/// `NetInfo(&mut [u8; 48]) -> 0 or 1`.
pub fn net_info(process: &Process, buf: u64) -> Result<u64, Error> {
    let info = {
        let guard = NET.lock();
        let Some(net) = guard.as_ref() else {
            return Ok(0);
        };
        let c = net.stack.config();
        let mut driver = [0u8; 16];
        let name = net.card.name().as_bytes();
        driver[..name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
        abi::net::NetInfo {
            driver,
            mac: net.stack.mac(),
            link: net.card.link_up(),
            configured: net.stack.configured(),
            ip: c.ip,
            mask: c.mask,
            gateway: c.gateway,
            dns: c.dns,
        }
    };
    process
        .space()
        .write(buf, &info.to_bytes())
        .map_err(|_| Error::BadAddress)?;
    Ok(1)
}

/// Waits up to `timeout_ns` for the interface to get an address (for the
/// self test and early users).
pub fn wait_configured(timeout_ns: u64) -> bool {
    let end = frame::time::now_ns() + timeout_ns;
    loop {
        if NET.lock().as_ref().is_some_and(|n| n.stack.configured()) {
            return true;
        }
        if frame::time::now_ns() >= end {
            return false;
        }
        task::sleep_ms(20);
    }
}
