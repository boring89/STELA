// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The network stack of one interface, without I/O: frames come in
//! through [`Stack::receive`], timers advance in [`Stack::poll`], the
//! frames to transmit are taken with [`Stack::take_output`]. Sockets are
//! numbered ([`SocketId`]); the kernel wraps them in handles.
//!
//! ```text
//!  receive(frame) ─► Ethernet ─┬─ ARP: learn, answer requests for our IP,
//!                              │       send packets that waited for it
//!                              └─ IPv4 (to us or broadcast) ─┬─ ICMP: answer echo, queue replies
//!                                                            ├─ UDP: DHCP client, sockets
//!                                                            └─ TCP: connections, listeners, else RST
//!  send: route (same subnet: direct, else the gateway) ─► ARP cache, or
//!        queue the packet and ask (ARP request, 1 s, 3 tries)
//! ```

use crate::dhcp;
use crate::tcp::{self, Incoming, State, Tcb, TcpError};
use crate::wire::{self, ANY, BROADCAST_IP, BROADCAST_MAC, Ip, Mac, tcp_flags};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// A socket's number in the stack.
pub type SocketId = usize;

/// What a socket is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Datagrams.
    Udp,
    /// A byte stream (or a listener).
    Tcp,
    /// ICMP echo (ping).
    Icmp,
}

/// Errors of socket operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetError {
    /// No such socket.
    BadSocket,
    /// The operation does not fit the socket's kind or state.
    InvalidState,
    /// The port is taken.
    PortInUse,
    /// The interface has no address yet (DHCP not done).
    Unconfigured,
    /// The peer refused the connection.
    Refused,
    /// The peer reset the connection.
    Reset,
    /// The peer stopped answering.
    TimedOut,
    /// The socket table is full.
    NoResources,
    /// A datagram larger than the MTU.
    TooLarge,
}

impl From<TcpError> for NetError {
    fn from(e: TcpError) -> NetError {
        match e {
            TcpError::Refused => NetError::Refused,
            TcpError::Reset => NetError::Reset,
            TcpError::TimedOut => NetError::TimedOut,
        }
    }
}

/// The interface's address configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Our address (0.0.0.0 until configured).
    pub ip: Ip,
    /// Subnet mask.
    pub mask: Ip,
    /// Default gateway.
    pub gateway: Ip,
    /// DNS server.
    pub dns: Ip,
}

/// Datagrams a UDP or ICMP socket keeps before dropping new ones.
const QUEUE: usize = 32;
/// Sockets at most.
const MAX_SOCKETS: usize = 256;
/// ARP cache entries live this long.
const ARP_TTL_MS: u64 = 300_000;
/// DHCP messages are retried this often.
const DHCP_RETRY_MS: u64 = 2000;
const FIRST_EPHEMERAL: u16 = 49152;

enum Sock {
    Udp {
        port: u16,
        peer: Option<(Ip, u16)>,
        queue: VecDeque<(Ip, u16, Vec<u8>)>,
    },
    Icmp {
        id: u16,
        queue: VecDeque<(Ip, u16, Vec<u8>)>,
    },
    /// TCP, before connect or listen.
    TcpIdle { port: u16 },
    Listen {
        port: u16,
        backlog: usize,
        ready: VecDeque<SocketId>,
    },
    /// A connection; `true` once the application closed it (the slot is
    /// freed when the protocol is done).
    Conn(Box<Tcb>, bool),
}

struct Pending {
    next_hop: Ip,
    packet: Vec<u8>,
    next_try: u64,
    tries: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dhcp {
    Off,
    Discovering {
        xid: u32,
        next_try: u64,
    },
    Requesting {
        xid: u32,
        offer: dhcp::Reply,
        next_try: u64,
    },
    Bound,
}

/// The stack.
pub struct Stack {
    mac: Mac,
    config: Config,
    dhcp: Dhcp,
    arp: Vec<(Ip, Mac, u64)>,
    pending: Vec<Pending>,
    sockets: Vec<Option<Sock>>,
    out: Vec<Vec<u8>>,
    ip_id: u16,
    next_port: u16,
    rand: u64,
    /// Something a socket waits for may have happened since the last
    /// [`Stack::take_events`].
    events: bool,
}

impl Stack {
    /// A stack for the interface with address `mac`; `seed` varies the
    /// sequence numbers, DHCP transaction ids and ephemeral ports.
    pub fn new(mac: Mac, seed: u64) -> Stack {
        Stack {
            mac,
            config: Config::default(),
            dhcp: Dhcp::Off,
            arp: Vec::new(),
            pending: Vec::new(),
            sockets: Vec::new(),
            out: Vec::new(),
            ip_id: seed as u16,
            next_port: FIRST_EPHEMERAL + (seed % 1000) as u16,
            rand: seed | 1,
            events: false,
        }
    }

    fn random(&mut self) -> u32 {
        // xorshift64 (Marsaglia 2003).
        self.rand ^= self.rand << 13;
        self.rand ^= self.rand >> 7;
        self.rand ^= self.rand << 17;
        self.rand as u32
    }

    /// The interface's MAC address.
    pub fn mac(&self) -> Mac {
        self.mac
    }

    /// The address configuration.
    pub fn config(&self) -> Config {
        self.config
    }

    /// Whether the interface has an address.
    pub fn configured(&self) -> bool {
        self.config.ip != ANY
    }

    /// Sets the address configuration by hand (and stops DHCP).
    pub fn configure(&mut self, config: Config) {
        self.config = config;
        self.dhcp = Dhcp::Bound;
        self.events = true;
    }

    /// Starts asking a DHCP server for an address.
    pub fn start_dhcp(&mut self, now: u64) {
        let xid = self.random();
        self.dhcp = Dhcp::Discovering { xid, next_try: 0 };
        self.poll_dhcp(now);
    }

    /// The frames to transmit, oldest first.
    pub fn take_output(&mut self) -> Vec<Vec<u8>> {
        core::mem::take(&mut self.out)
    }

    /// Whether anything a socket may wait for happened (data, a new
    /// connection, a state change) since the last call.
    pub fn take_events(&mut self) -> bool {
        core::mem::take(&mut self.events)
    }

    // ---- sending ----

    fn same_subnet(&self, ip: Ip) -> bool {
        (0..4).all(|i| ip[i] & self.config.mask[i] == self.config.ip[i] & self.config.mask[i])
    }

    /// Sends an IPv4 packet with `proto` and `payload` to `dst`.
    fn send_ip(&mut self, dst: Ip, proto: u8, payload: &[u8], now: u64) {
        self.ip_id = self.ip_id.wrapping_add(1);
        let packet = wire::build_ipv4(self.config.ip, dst, proto, self.ip_id, payload);
        let broadcast = dst == BROADCAST_IP
            || (self.configured()
                && (0..4).all(|i| dst[i] | self.config.mask[i] == 0xFF)
                && self.same_subnet(dst));
        if broadcast {
            self.out.push(wire::build_eth(
                BROADCAST_MAC,
                self.mac,
                wire::ETHERTYPE_IPV4,
                &packet,
            ));
            return;
        }
        let next_hop = if self.same_subnet(dst) {
            dst
        } else {
            self.config.gateway
        };
        if let Some(mac) = self.lookup(next_hop, now) {
            self.out.push(wire::build_eth(
                mac,
                self.mac,
                wire::ETHERTYPE_IPV4,
                &packet,
            ));
            return;
        }
        let asking = self.pending.iter().any(|p| p.next_hop == next_hop);
        self.pending.push(Pending {
            next_hop,
            packet,
            next_try: now + 1000,
            tries: 1,
        });
        if !asking {
            self.arp_request(next_hop);
        }
    }

    fn lookup(&self, ip: Ip, now: u64) -> Option<Mac> {
        self.arp
            .iter()
            .find(|(a, _, expires)| *a == ip && *expires > now)
            .map(|e| e.1)
    }

    fn arp_request(&mut self, ip: Ip) {
        let arp = wire::Arp {
            op: wire::ARP_REQUEST,
            sender_mac: self.mac,
            sender_ip: self.config.ip,
            target_mac: [0; 6],
            target_ip: ip,
        };
        self.out.push(wire::build_eth(
            BROADCAST_MAC,
            self.mac,
            wire::ETHERTYPE_ARP,
            &wire::build_arp(&arp),
        ));
    }

    fn learn(&mut self, ip: Ip, mac: Mac, now: u64) {
        if ip == ANY {
            return;
        }
        match self.arp.iter_mut().find(|e| e.0 == ip) {
            Some(e) => {
                e.1 = mac;
                e.2 = now + ARP_TTL_MS;
            }
            None => {
                if self.arp.len() >= 64 {
                    self.arp.remove(0);
                }
                self.arp.push((ip, mac, now + ARP_TTL_MS));
            }
        }
        // Packets that waited for this address.
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].next_hop == ip {
                let p = self.pending.remove(i);
                self.out.push(wire::build_eth(
                    mac,
                    self.mac,
                    wire::ETHERTYPE_IPV4,
                    &p.packet,
                ));
            } else {
                i += 1;
            }
        }
    }

    // ---- receiving ----

    /// Handles one received Ethernet frame.
    pub fn receive(&mut self, frame: &[u8], now: u64) {
        let Some(eth) = wire::parse_eth(frame) else {
            return;
        };
        if eth.dst != self.mac && eth.dst != BROADCAST_MAC {
            return;
        }
        match eth.ethertype {
            wire::ETHERTYPE_ARP => self.receive_arp(eth.payload, now),
            wire::ETHERTYPE_IPV4 => self.receive_ip(eth.payload, now),
            _ => {}
        }
    }

    fn receive_arp(&mut self, p: &[u8], now: u64) {
        let Some(arp) = wire::parse_arp(p) else {
            return;
        };
        let for_us = self.configured() && arp.target_ip == self.config.ip;
        if for_us || self.arp.iter().any(|e| e.0 == arp.sender_ip) || arp.op == wire::ARP_REPLY {
            self.learn(arp.sender_ip, arp.sender_mac, now);
        }
        if for_us && arp.op == wire::ARP_REQUEST {
            let reply = wire::Arp {
                op: wire::ARP_REPLY,
                sender_mac: self.mac,
                sender_ip: self.config.ip,
                target_mac: arp.sender_mac,
                target_ip: arp.sender_ip,
            };
            self.out.push(wire::build_eth(
                arp.sender_mac,
                self.mac,
                wire::ETHERTYPE_ARP,
                &wire::build_arp(&reply),
            ));
        }
    }

    fn receive_ip(&mut self, p: &[u8], now: u64) {
        let Some(ip) = wire::parse_ipv4(p) else {
            return;
        };
        let to_us = ip.dst == self.config.ip
            || ip.dst == BROADCAST_IP
            || !self.configured()
            || (self.same_subnet(ip.dst)
                && (0..4).all(|i| ip.dst[i] | self.config.mask[i] == 0xFF));
        if !to_us {
            return;
        }
        match ip.proto {
            wire::PROTO_ICMP => self.receive_icmp(ip.src, ip.payload, now),
            wire::PROTO_UDP => self.receive_udp(ip.src, ip.dst, ip.payload, now),
            wire::PROTO_TCP if self.configured() => {
                self.receive_tcp(ip.src, ip.dst, ip.payload, now)
            }
            _ => {}
        }
    }

    fn receive_icmp(&mut self, src: Ip, p: &[u8], now: u64) {
        let Some(echo) = wire::parse_echo(p) else {
            return;
        };
        if echo.kind == wire::ICMP_ECHO_REQUEST {
            if self.configured() {
                let reply = wire::build_echo(wire::ICMP_ECHO_REPLY, echo.id, echo.seq, echo.data);
                self.send_ip(src, wire::PROTO_ICMP, &reply, now);
            }
            return;
        }
        for s in self.sockets.iter_mut().flatten() {
            if let Sock::Icmp { id, queue } = s
                && *id == echo.id
                && queue.len() < QUEUE
            {
                queue.push_back((src, echo.seq, echo.data.to_vec()));
                self.events = true;
            }
        }
    }

    fn receive_udp(&mut self, src: Ip, dst: Ip, p: &[u8], now: u64) {
        let Some(udp) = wire::parse_udp(src, dst, p) else {
            return;
        };
        if udp.dst_port == dhcp::CLIENT_PORT && udp.src_port == dhcp::SERVER_PORT {
            self.receive_dhcp(udp.data, now);
            return;
        }
        for s in self.sockets.iter_mut().flatten() {
            if let Sock::Udp { port, queue, .. } = s
                && *port == udp.dst_port
            {
                if queue.len() < QUEUE {
                    queue.push_back((src, udp.src_port, udp.data.to_vec()));
                    self.events = true;
                }
                return;
            }
        }
    }

    fn receive_tcp(&mut self, src: Ip, dst: Ip, p: &[u8], now: u64) {
        let Some(t) = wire::parse_tcp(src, dst, p) else {
            return;
        };
        let seg = Incoming {
            seq: t.seq,
            ack: t.ack,
            flags: t.flags,
            window: t.window,
            mss: t.mss,
            data: t.data,
        };
        // An existing connection.
        let conn = self.sockets.iter().position(|s| {
            matches!(s, Some(Sock::Conn(c, _)) if c.local_port == t.dst_port
                && c.remote_port == t.src_port && c.remote_ip == src && !c.is_closed())
        });
        if let Some(id) = conn {
            let Some(Some(Sock::Conn(c, _))) = self.sockets.get_mut(id) else {
                return;
            };
            c.on_segment(&seg, now);
            self.events = true;
            self.flush_tcp(id);
            self.reap(id);
            return;
        }
        // A listener (SYN only).
        let listener = self
            .sockets
            .iter()
            .position(|s| matches!(s, Some(Sock::Listen { port, .. }) if *port == t.dst_port));
        if let Some(lid) = listener
            && t.flags & (tcp_flags::SYN | tcp_flags::ACK | tcp_flags::RST) == tcp_flags::SYN
        {
            let full = matches!(&self.sockets[lid],
                Some(Sock::Listen { backlog, ready, .. }) if ready.len() >= *backlog);
            if !full {
                let iss = self.random();
                let tcb = Tcb::accept(t.dst_port, src, t.src_port, &seg, iss, now);
                if let Ok(id) = self.insert(Sock::Conn(Box::new(tcb), false)) {
                    if let Some(Some(Sock::Listen { ready, .. })) = self.sockets.get_mut(lid) {
                        ready.push_back(id);
                    }
                    self.flush_tcp(id);
                }
            }
            return;
        }
        if t.flags & tcp_flags::RST != 0 {
            return;
        }
        // Nobody there: reset (RFC 793 3.4, "reset generation").
        let (seq, ack, flags) = if t.flags & tcp_flags::ACK != 0 {
            (t.ack, 0, tcp_flags::RST)
        } else {
            let len = t.data.len() as u32
                + u32::from(t.flags & tcp_flags::SYN != 0)
                + u32::from(t.flags & tcp_flags::FIN != 0);
            (0, t.seq.wrapping_add(len), tcp_flags::RST | tcp_flags::ACK)
        };
        let rst = wire::build_tcp(
            self.config.ip,
            src,
            &wire::TcpOut {
                src_port: t.dst_port,
                dst_port: t.src_port,
                seq,
                ack,
                flags,
                window: 0,
                mss: None,
                data: &[],
            },
        );
        self.send_ip(src, wire::PROTO_TCP, &rst, now);
    }

    /// Sends the segments connection `id` queued.
    fn flush_tcp(&mut self, id: SocketId) {
        let Some(Some(Sock::Conn(c, _))) = self.sockets.get_mut(id) else {
            return;
        };
        let segs = core::mem::take(&mut c.out);
        let (lp, rip, rp) = (c.local_port, c.remote_ip, c.remote_port);
        for s in segs {
            let p = wire::build_tcp(
                self.config.ip,
                rip,
                &wire::TcpOut {
                    src_port: lp,
                    dst_port: rp,
                    seq: s.seq,
                    ack: s.ack,
                    flags: s.flags,
                    window: s.window,
                    mss: s.mss,
                    data: &s.data,
                },
            );
            // The time only matters for ARP retries, which `poll` drives.
            self.send_ip(rip, wire::PROTO_TCP, &p, 0);
        }
    }

    // ---- DHCP ----

    fn send_dhcp(&mut self, kind: u8, xid: u32, requested: Option<Ip>, server: Option<Ip>) {
        let msg = dhcp::build(kind, xid, self.mac, requested, server);
        let udp = wire::build_udp(
            ANY,
            BROADCAST_IP,
            dhcp::CLIENT_PORT,
            dhcp::SERVER_PORT,
            &msg,
        );
        self.ip_id = self.ip_id.wrapping_add(1);
        let ip = wire::build_ipv4(ANY, BROADCAST_IP, wire::PROTO_UDP, self.ip_id, &udp);
        self.out.push(wire::build_eth(
            BROADCAST_MAC,
            self.mac,
            wire::ETHERTYPE_IPV4,
            &ip,
        ));
    }

    fn poll_dhcp(&mut self, now: u64) {
        match self.dhcp {
            Dhcp::Discovering { xid, next_try } if now >= next_try => {
                self.send_dhcp(dhcp::DISCOVER, xid, None, None);
                self.dhcp = Dhcp::Discovering {
                    xid,
                    next_try: now + DHCP_RETRY_MS,
                };
            }
            Dhcp::Requesting {
                xid,
                offer,
                next_try,
            } if now >= next_try => {
                self.send_dhcp(dhcp::REQUEST, xid, Some(offer.your_ip), Some(offer.server));
                self.dhcp = Dhcp::Requesting {
                    xid,
                    offer,
                    next_try: now + DHCP_RETRY_MS,
                };
            }
            _ => {}
        }
    }

    fn receive_dhcp(&mut self, p: &[u8], now: u64) {
        let Some(reply) = dhcp::parse(p, self.mac) else {
            return;
        };
        match self.dhcp {
            Dhcp::Discovering { xid, .. } if reply.xid == xid && reply.kind == dhcp::OFFER => {
                self.dhcp = Dhcp::Requesting {
                    xid,
                    offer: reply,
                    next_try: 0,
                };
                self.poll_dhcp(now);
            }
            Dhcp::Requesting { xid, offer, .. } if reply.xid == xid => {
                if reply.kind == dhcp::ACK {
                    let server = if reply.server == ANY {
                        offer.server
                    } else {
                        reply.server
                    };
                    self.config = Config {
                        ip: reply.your_ip,
                        mask: if reply.mask == ANY {
                            [255, 255, 255, 0]
                        } else {
                            reply.mask
                        },
                        gateway: if reply.router == ANY {
                            server
                        } else {
                            reply.router
                        },
                        dns: reply.dns,
                    };
                    self.dhcp = Dhcp::Bound;
                    self.events = true;
                } else if reply.kind == dhcp::NAK {
                    self.start_dhcp(now);
                }
            }
            _ => {}
        }
    }

    // ---- timers ----

    /// Advances the timers to `now` (milliseconds): DHCP and ARP retries,
    /// TCP retransmissions, closed connections.
    pub fn poll(&mut self, now: u64) {
        self.poll_dhcp(now);
        // ARP: ask again, or give up on the packets after three tries.
        let mut ask = Vec::new();
        self.pending.retain_mut(|p| {
            if now < p.next_try {
                return true;
            }
            if p.tries >= 3 {
                return false;
            }
            p.tries += 1;
            p.next_try = now + 1000;
            if !ask.contains(&p.next_hop) {
                ask.push(p.next_hop);
            }
            true
        });
        for ip in ask {
            self.arp_request(ip);
        }
        for id in 0..self.sockets.len() {
            if let Some(Some(Sock::Conn(c, _))) = self.sockets.get_mut(id) {
                let before = c.state;
                c.poll(now);
                if c.state != before {
                    self.events = true;
                }
                self.flush_tcp(id);
                self.reap(id);
            }
        }
    }

    // ---- sockets ----

    fn insert(&mut self, sock: Sock) -> Result<SocketId, NetError> {
        if let Some(i) = self.sockets.iter().position(Option::is_none) {
            self.sockets[i] = Some(sock);
            return Ok(i);
        }
        if self.sockets.len() >= MAX_SOCKETS {
            return Err(NetError::NoResources);
        }
        self.sockets.push(Some(sock));
        Ok(self.sockets.len() - 1)
    }

    fn sock(&mut self, id: SocketId) -> Result<&mut Sock, NetError> {
        self.sockets
            .get_mut(id)
            .and_then(Option::as_mut)
            .ok_or(NetError::BadSocket)
    }

    fn port_taken(&self, port: u16, kind: Kind) -> bool {
        self.sockets.iter().flatten().any(|s| match (s, kind) {
            (Sock::Udp { port: p, .. }, Kind::Udp) => *p == port,
            (Sock::TcpIdle { port: p }, Kind::Tcp) | (Sock::Listen { port: p, .. }, Kind::Tcp) => {
                *p == port && port != 0
            }
            (Sock::Conn(c, _), Kind::Tcp) => c.local_port == port && !c.is_closed(),
            _ => false,
        })
    }

    fn ephemeral(&mut self, kind: Kind) -> u16 {
        loop {
            let p = self.next_port;
            self.next_port = if p == u16::MAX {
                FIRST_EPHEMERAL
            } else {
                p + 1
            };
            if !self.port_taken(p, kind) {
                return p;
            }
        }
    }

    /// A new socket.
    pub fn socket(&mut self, kind: Kind) -> Result<SocketId, NetError> {
        let sock = match kind {
            Kind::Udp => Sock::Udp {
                port: 0,
                peer: None,
                queue: VecDeque::new(),
            },
            Kind::Tcp => Sock::TcpIdle { port: 0 },
            Kind::Icmp => {
                let id = self.random() as u16;
                Sock::Icmp {
                    id,
                    queue: VecDeque::new(),
                }
            }
        };
        self.insert(sock)
    }

    /// Gives the socket local port `port` (0: an ephemeral one).
    pub fn bind(&mut self, id: SocketId, port: u16) -> Result<u16, NetError> {
        let kind = match self.sock(id)? {
            Sock::Udp { port: 0, .. } => Kind::Udp,
            Sock::TcpIdle { port: 0 } => Kind::Tcp,
            _ => return Err(NetError::InvalidState),
        };
        let port = if port == 0 {
            self.ephemeral(kind)
        } else if self.port_taken(port, kind) {
            return Err(NetError::PortInUse);
        } else {
            port
        };
        match self.sock(id)? {
            Sock::Udp { port: p, .. } | Sock::TcpIdle { port: p } => *p = port,
            _ => return Err(NetError::InvalidState),
        }
        Ok(port)
    }

    /// The socket's local port (0 if none yet).
    pub fn local_port(&mut self, id: SocketId) -> Result<u16, NetError> {
        Ok(match self.sock(id)? {
            Sock::Udp { port, .. } | Sock::TcpIdle { port } | Sock::Listen { port, .. } => *port,
            Sock::Conn(c, _) => c.local_port,
            Sock::Icmp { .. } => 0,
        })
    }

    /// TCP: starts connecting to `ip:port` (wait with
    /// [`Stack::connected`]). UDP: sets the default destination.
    pub fn connect(&mut self, id: SocketId, ip: Ip, port: u16, now: u64) -> Result<(), NetError> {
        if !self.configured() {
            return Err(NetError::Unconfigured);
        }
        match self.sock(id)? {
            Sock::Udp { peer, .. } => {
                *peer = Some((ip, port));
                return Ok(());
            }
            Sock::TcpIdle { .. } => {}
            _ => return Err(NetError::InvalidState),
        }
        let local = match self.sock(id)? {
            Sock::TcpIdle { port: 0 } => self.ephemeral(Kind::Tcp),
            Sock::TcpIdle { port } => *port,
            _ => return Err(NetError::InvalidState),
        };
        let iss = self.random();
        let tcb = Tcb::connect(local, ip, port, iss, now);
        *self.sock(id)? = Sock::Conn(Box::new(tcb), false);
        self.flush_tcp(id);
        Ok(())
    }

    /// TCP: `Ok(true)` once established, `Ok(false)` while connecting,
    /// the error if it failed.
    pub fn connected(&mut self, id: SocketId) -> Result<bool, NetError> {
        match self.sock(id)? {
            Sock::Conn(c, _) => match (c.state, c.error) {
                (_, Some(e)) => Err(e.into()),
                (State::SynSent | State::SynReceived, _) => Ok(false),
                (State::Closed, _) => Err(NetError::Reset),
                _ => Ok(true),
            },
            _ => Err(NetError::InvalidState),
        }
    }

    /// TCP: listens on the bound port with room for `backlog`
    /// connections not yet accepted.
    pub fn listen(&mut self, id: SocketId, backlog: usize) -> Result<(), NetError> {
        let port = match self.sock(id)? {
            Sock::TcpIdle { port } if *port != 0 => *port,
            _ => return Err(NetError::InvalidState),
        };
        *self.sock(id)? = Sock::Listen {
            port,
            backlog: backlog.clamp(1, 64),
            ready: VecDeque::new(),
        };
        Ok(())
    }

    /// TCP: an established connection from the listener's queue, if any.
    pub fn accept(&mut self, id: SocketId) -> Result<Option<SocketId>, NetError> {
        let ready = match self.sock(id)? {
            Sock::Listen { ready, .. } => ready.clone(),
            _ => return Err(NetError::InvalidState),
        };
        for (i, conn) in ready.iter().enumerate() {
            match self.connected(*conn) {
                Ok(true) => {
                    if let Sock::Listen { ready, .. } = self.sock(id)? {
                        ready.remove(i);
                    }
                    return Ok(Some(*conn));
                }
                Ok(false) => {}
                Err(_) => {
                    // Failed during the handshake: forget it.
                    self.sockets[*conn] = None;
                    if let Sock::Listen { ready, .. } = self.sock(id)? {
                        ready.remove(i);
                    }
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    /// Sends `data`: TCP queues it (returns how much fit); UDP sends one
    /// datagram to `to` (or the connected peer); ICMP sends an echo
    /// request with sequence number `to.1` to `to.0`.
    pub fn send(
        &mut self,
        id: SocketId,
        data: &[u8],
        to: Option<(Ip, u16)>,
        now: u64,
    ) -> Result<usize, NetError> {
        match self.sock(id)? {
            Sock::Conn(c, _) => {
                if let Some(e) = c.error {
                    return Err(e.into());
                }
                if !c.can_send() {
                    return Err(NetError::InvalidState);
                }
                let n = c.send(data, now);
                self.flush_tcp(id);
                Ok(n)
            }
            Sock::Udp { port, peer, .. } => {
                let dest = to.or(*peer).ok_or(NetError::InvalidState)?;
                let p = *port;
                if data.len() > wire::MTU - wire::IP_HEADER - 8 {
                    return Err(NetError::TooLarge);
                }
                if !self.configured() {
                    return Err(NetError::Unconfigured);
                }
                let src_port = if p == 0 { self.bind(id, 0)? } else { p };
                let udp = wire::build_udp(self.config.ip, dest.0, src_port, dest.1, data);
                self.send_ip(dest.0, wire::PROTO_UDP, &udp, now);
                Ok(data.len())
            }
            Sock::Icmp { id: icmp_id, .. } => {
                let icmp_id = *icmp_id;
                let (ip, seq) = to.ok_or(NetError::InvalidState)?;
                if !self.configured() {
                    return Err(NetError::Unconfigured);
                }
                if data.len() > wire::MTU - wire::IP_HEADER - 8 {
                    return Err(NetError::TooLarge);
                }
                let echo = wire::build_echo(wire::ICMP_ECHO_REQUEST, icmp_id, seq, data);
                self.send_ip(ip, wire::PROTO_ICMP, &echo, now);
                Ok(data.len())
            }
            _ => Err(NetError::InvalidState),
        }
    }

    /// Receives: `Ok(None)` when nothing is there yet; TCP gives
    /// `Some((0, ..))` at the end of the stream; UDP and ICMP give one
    /// datagram (cut to `buf`) with its source (ICMP: the sequence number
    /// as the port).
    pub fn recv(
        &mut self,
        id: SocketId,
        buf: &mut [u8],
    ) -> Result<Option<(usize, Ip, u16)>, NetError> {
        let result = match self.sock(id)? {
            Sock::Conn(c, _) => {
                let (ip, port) = (c.remote_ip, c.remote_port);
                if c.readable() > 0 {
                    let n = c.recv(buf);
                    Some((n, ip, port))
                } else if c.at_end() {
                    match c.error {
                        Some(TcpError::Reset) => return Err(NetError::Reset),
                        Some(TcpError::TimedOut) => return Err(NetError::TimedOut),
                        _ => Some((0, ip, port)),
                    }
                } else if matches!(c.state, State::SynSent | State::SynReceived) {
                    None
                } else if c.is_closed() {
                    Some((0, ip, port))
                } else {
                    None
                }
            }
            Sock::Udp { queue, .. } | Sock::Icmp { queue, .. } => {
                queue.pop_front().map(|(ip, port, data)| {
                    let n = data.len().min(buf.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    (n, ip, port)
                })
            }
            _ => return Err(NetError::InvalidState),
        };
        self.flush_tcp(id);
        Ok(result)
    }

    /// Whether `recv` (or `accept`) would return something now.
    pub fn readable(&mut self, id: SocketId) -> bool {
        match self.sock(id) {
            Ok(Sock::Conn(c, _)) => c.readable() > 0 || c.at_end() || c.is_closed(),
            Ok(Sock::Udp { queue, .. } | Sock::Icmp { queue, .. }) => !queue.is_empty(),
            Ok(Sock::Listen { .. }) => true,
            _ => true,
        }
    }

    /// What the socket is ready for, as `abi::wait`-style flags
    /// ([`Readiness`]): data (or a connection, or the end) to read, room
    /// to send, established, closed or failed.
    pub fn readiness(&mut self, id: SocketId) -> Readiness {
        let mut r = Readiness::default();
        let ready_conns = match self.sock(id) {
            Ok(Sock::Conn(c, _)) => {
                let handshake = matches!(c.state, State::SynSent | State::SynReceived);
                r.readable = c.readable() > 0 || c.at_end() || c.is_closed();
                r.writable = !handshake && c.can_send() && c.send_room() > 0;
                r.connected = !handshake && c.error.is_none() && !c.is_closed();
                r.closed = c.error.is_some() || c.is_closed() || c.at_end();
                return r;
            }
            Ok(Sock::Udp { queue, .. } | Sock::Icmp { queue, .. }) => {
                r.readable = !queue.is_empty();
                r.writable = true;
                return r;
            }
            Ok(Sock::TcpIdle { .. }) => return r,
            Ok(Sock::Listen { ready, .. }) => ready.clone(),
            Err(_) => {
                r.closed = true;
                return r;
            }
        };
        r.readable = ready_conns
            .iter()
            .any(|&c| !matches!(self.connected(c), Ok(false)));
        r
    }

    /// Closes the socket: a TCP connection sends FIN (and lingers until
    /// it is done); everything else goes at once.
    pub fn close(&mut self, id: SocketId, now: u64) {
        let Some(slot) = self.sockets.get_mut(id) else {
            return;
        };
        match slot {
            Some(Sock::Conn(c, closed)) => {
                c.close(now);
                *closed = true;
                self.flush_tcp(id);
                self.reap(id);
            }
            Some(Sock::Listen { ready, .. }) => {
                let pending: Vec<SocketId> = ready.drain(..).collect();
                *slot = None;
                for conn in pending {
                    if let Some(Some(Sock::Conn(c, _))) = self.sockets.get_mut(conn) {
                        c.abort();
                    }
                    self.flush_tcp(conn);
                    self.sockets[conn] = None;
                }
            }
            _ => *slot = None,
        }
    }

    /// Connections closed by the application stay until the protocol is
    /// done with them; then their slot is freed.
    fn reap(&mut self, id: SocketId) {
        if let Some(Some(Sock::Conn(c, true))) = self.sockets.get(id)
            && c.is_closed()
        {
            self.sockets[id] = None;
        }
    }

    /// TCP: the application will send no more (FIN after the queued
    /// data) but keeps reading.
    pub fn shutdown(&mut self, id: SocketId, now: u64) -> Result<(), NetError> {
        match self.sock(id)? {
            Sock::Conn(c, _) => c.close(now),
            _ => return Err(NetError::InvalidState),
        }
        self.flush_tcp(id);
        Ok(())
    }

    /// The connected peer (TCP connections, connected UDP sockets).
    pub fn peer(&mut self, id: SocketId) -> Option<(Ip, u16)> {
        match self.sock(id).ok()? {
            Sock::Conn(c, _) => Some((c.remote_ip, c.remote_port)),
            Sock::Udp { peer, .. } => *peer,
            _ => None,
        }
    }

    /// Number of sockets in use (for tests and diagnostics).
    pub fn socket_count(&self) -> usize {
        self.sockets.iter().flatten().count()
    }

    /// The TCP state of a connection (diagnostics).
    pub fn tcp_state(&mut self, id: SocketId) -> Option<State> {
        match self.sock(id) {
            Ok(Sock::Conn(c, _)) => Some(c.state),
            _ => None,
        }
    }
}

/// What a socket is ready for ([`Stack::readiness`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Readiness {
    /// `recv` (or `accept`) would return at once.
    pub readable: bool,
    /// `send` would take some data.
    pub writable: bool,
    /// A TCP connection is established.
    pub connected: bool,
    /// The connection ended, was reset or failed.
    pub closed: bool,
}

/// The TCP buffer size, for callers sizing their reads.
pub const TCP_BUFFER: usize = tcp::BUFFER;
