// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Two stacks on one simulated wire: ARP, ping, UDP, TCP; and a stack
//! against a scripted DHCP server.

use crate::stack::{Config, Kind, NetError, Stack};
use crate::tcp::State;
use crate::{dhcp, wire};
use alloc::vec::Vec;

const A_MAC: wire::Mac = [0x52, 0x54, 0, 0, 0, 0xA];
const B_MAC: wire::Mac = [0x52, 0x54, 0, 0, 0, 0xB];
const A_IP: wire::Ip = [10, 0, 2, 15];
const B_IP: wire::Ip = [10, 0, 2, 2];

fn config(ip: wire::Ip) -> Config {
    Config {
        ip,
        mask: [255, 255, 255, 0],
        gateway: B_IP,
        dns: [10, 0, 2, 3],
    }
}

fn pair() -> (Stack, Stack) {
    let mut a = Stack::new(A_MAC, 1);
    let mut b = Stack::new(B_MAC, 2);
    a.configure(config(A_IP));
    b.configure(config(B_IP));
    (a, b)
}

/// Moves frames between the stacks until both are quiet; returns how
/// many moved.
fn exchange(a: &mut Stack, b: &mut Stack, now: u64) -> usize {
    let mut moved = 0;
    loop {
        let fa = a.take_output();
        let fb = b.take_output();
        if fa.is_empty() && fb.is_empty() {
            return moved;
        }
        moved += fa.len() + fb.len();
        for f in fa {
            b.receive(&f, now);
        }
        for f in fb {
            a.receive(&f, now);
        }
    }
}

#[test]
fn arp_then_ping() {
    let (mut a, mut b) = pair();
    let ping = a.socket(Kind::Icmp).expect("icmp socket");
    a.send(ping, b"stela", Some((B_IP, 1)), 0).expect("send");
    // First an ARP request (broadcast), the packet waits.
    let out = a.take_output();
    assert_eq!(out.len(), 1);
    let eth = wire::parse_eth(&out[0]).expect("frame");
    assert_eq!(eth.ethertype, wire::ETHERTYPE_ARP);
    assert_eq!(eth.dst, wire::BROADCAST_MAC);
    for f in out {
        b.receive(&f, 0);
    }
    exchange(&mut a, &mut b, 0);
    let mut buf = [0u8; 16];
    let (n, from, seq) = a.recv(ping, &mut buf).expect("recv").expect("a reply");
    assert_eq!((&buf[..n], from, seq), (&b"stela"[..], B_IP, 1));
    // The address is cached now: the next ping goes out at once.
    a.send(ping, b"again", Some((B_IP, 2)), 1).expect("send");
    let out = a.take_output();
    assert_eq!(
        wire::parse_eth(&out[0]).map(|e| e.ethertype),
        Some(wire::ETHERTYPE_IPV4)
    );
}

#[test]
fn arp_gives_up() {
    let mut a = Stack::new(A_MAC, 1);
    a.configure(config(A_IP));
    let u = a.socket(Kind::Udp).expect("socket");
    a.send(u, b"x", Some(([10, 0, 2, 99], 9)), 0).expect("send");
    assert_eq!(a.take_output().len(), 1);
    a.poll(1000);
    assert_eq!(a.take_output().len(), 1, "second ARP request");
    a.poll(2000);
    assert_eq!(a.take_output().len(), 1, "third ARP request");
    a.poll(3000);
    assert!(a.take_output().is_empty(), "given up");
}

#[test]
fn packets_off_the_subnet_go_to_the_gateway() {
    let (mut a, mut b) = pair();
    let u = a.socket(Kind::Udp).expect("socket");
    a.send(u, b"far", Some(([93, 184, 216, 34], 80)), 0)
        .expect("send");
    let out = a.take_output();
    let arp = wire::parse_arp(wire::parse_eth(&out[0]).expect("eth").payload).expect("arp");
    assert_eq!(arp.target_ip, B_IP);
    let _ = &mut b;
}

#[test]
fn udp_both_ways() {
    let (mut a, mut b) = pair();
    let server = b.socket(Kind::Udp).expect("socket");
    assert_eq!(b.bind(server, 7), Ok(7));
    let client = a.socket(Kind::Udp).expect("socket");
    a.send(client, b"echo me", Some((B_IP, 7)), 0)
        .expect("send");
    exchange(&mut a, &mut b, 0);
    let mut buf = [0u8; 32];
    let (n, from, port) = b.recv(server, &mut buf).expect("recv").expect("datagram");
    assert_eq!((&buf[..n], from), (&b"echo me"[..], A_IP));
    assert!(port >= 49152);
    b.send(server, &buf[..n], Some((from, port)), 0)
        .expect("reply");
    exchange(&mut a, &mut b, 0);
    let (n, _, src) = a.recv(client, &mut buf).expect("recv").expect("reply");
    assert_eq!((&buf[..n], src), (&b"echo me"[..], 7));
    // The port is taken; a datagram to a closed port is dropped.
    let other = b.socket(Kind::Udp).expect("socket");
    assert_eq!(b.bind(other, 7), Err(NetError::PortInUse));
    assert_eq!(a.recv(client, &mut buf), Ok(None));
}

#[test]
fn tcp_connect_transfer_close() {
    let (mut a, mut b) = pair();
    let listener = b.socket(Kind::Tcp).expect("socket");
    b.bind(listener, 80).expect("bind");
    b.listen(listener, 4).expect("listen");
    let c = a.socket(Kind::Tcp).expect("socket");
    a.connect(c, B_IP, 80, 0).expect("connect");
    assert_eq!(a.connected(c), Ok(false));
    exchange(&mut a, &mut b, 1);
    assert_eq!(a.connected(c), Ok(true));
    let s = b.accept(listener).expect("accept").expect("a connection");
    assert_eq!(b.accept(listener), Ok(None));
    a.send(c, b"GET /hello.txt HTTP/1.0\r\n\r\n", None, 2)
        .expect("send");
    exchange(&mut a, &mut b, 2);
    let mut buf = [0u8; 256];
    let (n, ..) = b.recv(s, &mut buf).expect("recv").expect("request");
    assert!(buf[..n].starts_with(b"GET /hello.txt"));
    let body: Vec<u8> = (0..10_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    assert_eq!(b.send(s, &body, None, 3), Ok(10_000));
    b.close(s, 3);
    exchange(&mut a, &mut b, 3);
    let mut got = Vec::new();
    loop {
        match a.recv(c, &mut buf).expect("recv") {
            Some((0, ..)) => break,
            Some((n, ..)) => got.extend_from_slice(&buf[..n]),
            None => panic!("data missing"),
        }
    }
    assert_eq!(got, body);
    a.close(c, 4);
    exchange(&mut a, &mut b, 4);
    // The server closed first: it lingers in TIME-WAIT; the client's
    // connection is done and its slot free.
    assert_eq!(b.tcp_state(s), Some(State::TimeWait));
    assert_eq!(a.tcp_state(c), None);
    b.close(s, 4);
    b.poll(10_000);
    // Both sides are gone; the listener stays.
    assert_eq!(a.socket_count(), 0);
    assert_eq!(b.socket_count(), 1);
}

#[test]
fn tcp_refused_by_a_closed_port() {
    let (mut a, mut b) = pair();
    let c = a.socket(Kind::Tcp).expect("socket");
    a.connect(c, B_IP, 81, 0).expect("connect");
    exchange(&mut a, &mut b, 0);
    assert_eq!(a.connected(c), Err(NetError::Refused));
}

#[test]
fn tcp_survives_a_lost_segment() {
    let (mut a, mut b) = pair();
    let l = b.socket(Kind::Tcp).expect("socket");
    b.bind(l, 80).expect("bind");
    b.listen(l, 1).expect("listen");
    let c = a.socket(Kind::Tcp).expect("socket");
    a.connect(c, B_IP, 80, 0).expect("connect");
    exchange(&mut a, &mut b, 0);
    let s = b.accept(l).expect("accept").expect("conn");
    a.send(c, b"lost", None, 10).expect("send");
    // The wire drops it.
    assert_eq!(a.take_output().len(), 1);
    a.poll(1010);
    exchange(&mut a, &mut b, 1010);
    let mut buf = [0u8; 8];
    assert_eq!(b.recv(s, &mut buf).expect("recv").map(|r| r.0), Some(4));
}

#[test]
fn unconfigured_stack_refuses_to_send() {
    let mut a = Stack::new(A_MAC, 1);
    let u = a.socket(Kind::Udp).expect("socket");
    assert_eq!(
        a.send(u, b"x", Some((B_IP, 1)), 0),
        Err(NetError::Unconfigured)
    );
    let t = a.socket(Kind::Tcp).expect("socket");
    assert_eq!(a.connect(t, B_IP, 1, 0), Err(NetError::Unconfigured));
}

/// Plays the DHCP server: answers the client's DISCOVER and REQUEST.
fn server_reply(frame: &[u8], kind: u8) -> Option<Vec<u8>> {
    let eth = wire::parse_eth(frame)?;
    let ip = wire::parse_ipv4(eth.payload)?;
    let udp = wire::parse_udp(ip.src, ip.dst, ip.payload)?;
    if udp.dst_port != dhcp::SERVER_PORT {
        return None;
    }
    let req = udp.data;
    let mut p = Vec::new();
    p.extend_from_slice(&[2, 1, 6, 0]);
    p.extend_from_slice(&req[4..8]); // xid
    p.extend_from_slice(&[0; 8]);
    p.extend_from_slice(&A_IP); // yiaddr
    p.extend_from_slice(&[0; 8]);
    p.extend_from_slice(&req[28..44]); // chaddr
    p.extend_from_slice(&[0; 192]);
    p.extend_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    p.extend_from_slice(&[53, 1, kind, 54, 4, 10, 0, 2, 2, 1, 4, 255, 255, 255, 0]);
    p.extend_from_slice(&[3, 4, 10, 0, 2, 2, 6, 4, 10, 0, 2, 3, 255]);
    let udp = wire::build_udp(
        B_IP,
        wire::BROADCAST_IP,
        dhcp::SERVER_PORT,
        dhcp::CLIENT_PORT,
        &p,
    );
    let ip = wire::build_ipv4(B_IP, wire::BROADCAST_IP, wire::PROTO_UDP, 1, &udp);
    Some(wire::build_eth(
        wire::BROADCAST_MAC,
        B_MAC,
        wire::ETHERTYPE_IPV4,
        &ip,
    ))
}

#[test]
fn dhcp_gets_an_address() {
    let mut a = Stack::new(A_MAC, 7);
    a.start_dhcp(0);
    let discover = a.take_output();
    assert_eq!(discover.len(), 1);
    // No answer: DISCOVER again after the retry time.
    a.poll(1000);
    assert!(a.take_output().is_empty());
    a.poll(2000);
    let discover = a.take_output();
    assert_eq!(discover.len(), 1);
    let offer = server_reply(&discover[0], dhcp::OFFER).expect("a DISCOVER");
    a.receive(&offer, 2001);
    let request = a.take_output();
    assert_eq!(request.len(), 1);
    assert!(!a.configured());
    let ack = server_reply(&request[0], dhcp::ACK).expect("a REQUEST");
    a.receive(&ack, 2002);
    assert!(a.configured());
    assert!(a.take_events());
    assert_eq!(a.config(), config(A_IP));
}

#[test]
fn tcp_shutdown_keeps_reading() {
    let (mut a, mut b) = pair();
    let l = b.socket(Kind::Tcp).expect("socket");
    b.bind(l, 80).expect("bind");
    b.listen(l, 1).expect("listen");
    let c = a.socket(Kind::Tcp).expect("socket");
    a.connect(c, B_IP, 80, 0).expect("connect");
    exchange(&mut a, &mut b, 0);
    let s = b.accept(l).expect("accept").expect("conn");
    a.send(c, b"request", None, 1).expect("send");
    a.shutdown(c, 1).expect("shutdown");
    exchange(&mut a, &mut b, 1);
    let mut buf = [0u8; 16];
    assert_eq!(b.recv(s, &mut buf).expect("recv").map(|r| r.0), Some(7));
    assert_eq!(b.recv(s, &mut buf).expect("recv").map(|r| r.0), Some(0));
    b.send(s, b"answer", None, 2).expect("send");
    exchange(&mut a, &mut b, 2);
    assert_eq!(a.recv(c, &mut buf).expect("recv").map(|r| r.0), Some(6));
}

#[test]
fn readiness_follows_the_connection() {
    let (mut a, mut b) = pair();
    let listener = b.socket(Kind::Tcp).expect("socket");
    b.bind(listener, 80).expect("bind");
    b.listen(listener, 4).expect("listen");
    assert!(!b.readiness(listener).readable);
    let c = a.socket(Kind::Tcp).expect("socket");
    a.connect(c, B_IP, 80, 0).expect("connect");
    let r = a.readiness(c);
    assert!(!r.connected && !r.writable && !r.readable);
    exchange(&mut a, &mut b, 1);
    let r = a.readiness(c);
    assert!(r.connected && r.writable && !r.readable && !r.closed);
    assert!(b.readiness(listener).readable);
    let s = b.accept(listener).expect("accept").expect("a connection");
    assert!(!b.readiness(listener).readable);
    b.send(s, b"hi", None, 2).expect("send");
    exchange(&mut a, &mut b, 2);
    assert!(a.readiness(c).readable);
    let mut buf = [0u8; 16];
    a.recv(c, &mut buf).expect("recv");
    assert!(!a.readiness(c).readable);
    b.close(s, 3);
    exchange(&mut a, &mut b, 3);
    let r = a.readiness(c);
    assert!(r.readable && r.closed);
    let u = a.socket(Kind::Udp).expect("socket");
    let r = a.readiness(u);
    assert!(r.writable && !r.readable);
}

/// A lossy, slow wire: frames from `b` to `a` take `delay` ms, and every
/// `drop_every`th of them (counting from 1) is lost.
struct Wire {
    in_flight: Vec<(u64, bool, Vec<u8>)>,
    count: u64,
}

impl Wire {
    fn step(&mut self, a: &mut Stack, b: &mut Stack, now: u64, delay: u64, drop_every: u64) {
        for f in a.take_output() {
            self.in_flight.push((now + delay, true, f));
        }
        for f in b.take_output() {
            self.count += 1;
            if self.count.is_multiple_of(drop_every) {
                continue;
            }
            self.in_flight.push((now + delay, false, f));
        }
        let due: Vec<(u64, bool, Vec<u8>)> = self
            .in_flight
            .iter()
            .filter(|(t, ..)| *t <= now)
            .cloned()
            .collect();
        self.in_flight.retain(|(t, ..)| *t > now);
        for (_, to_b, f) in due {
            if to_b {
                b.receive(&f, now);
            } else {
                a.receive(&f, now);
            }
        }
        a.poll(now);
        b.poll(now);
    }
}

#[test]
fn tcp_bulk_transfer_over_a_lossy_slow_wire() {
    let (mut a, mut b) = pair();
    let l = b.socket(Kind::Tcp).expect("socket");
    b.bind(l, 80).expect("bind");
    b.listen(l, 1).expect("listen");
    let c = a.socket(Kind::Tcp).expect("socket");
    a.connect(c, B_IP, 80, 0).expect("connect");
    exchange(&mut a, &mut b, 0);
    let s = b.accept(l).expect("accept").expect("conn");
    let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut sent = 0;
    let mut got = Vec::new();
    let mut wire = Wire {
        in_flight: Vec::new(),
        count: 0,
    };
    let mut buf = [0u8; 8192];
    let mut now = 1;
    // 20 ms each way, one data frame in 25 lost.
    while got.len() < body.len() && now < 120_000 {
        if sent < body.len() {
            sent += b.send(s, &body[sent..], None, now).expect("send");
        }
        wire.step(&mut a, &mut b, now, 20, 25);
        while let Some((n, ..)) = a.recv(c, &mut buf).expect("recv") {
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        now += 5;
    }
    assert_eq!(
        got.len(),
        body.len(),
        "only {} bytes by {now} ms",
        got.len()
    );
    assert_eq!(got, body);
    // Fast retransmit and reassembly keep it moving: well under the
    // time one RTO per loss would take (about 9 losses x 1 s).
    assert!(now < 20_000, "took {now} ms");
}
