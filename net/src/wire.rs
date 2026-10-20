// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Packet formats: building and parsing Ethernet frames, ARP, IPv4, ICMP
//! echo, UDP and TCP segments. All multi-byte fields are big-endian
//! ("network order").

use alloc::vec::Vec;

/// An IPv4 address.
pub type Ip = [u8; 4];
/// An Ethernet (MAC) address.
pub type Mac = [u8; 6];

/// The Ethernet broadcast address.
pub const BROADCAST_MAC: Mac = [0xFF; 6];
/// 0.0.0.0.
pub const ANY: Ip = [0; 4];
/// 255.255.255.255.
pub const BROADCAST_IP: Ip = [0xFF; 4];

/// EtherType of IPv4 (IEEE 802.3, RFC 894).
pub const ETHERTYPE_IPV4: u16 = 0x0800;
/// EtherType of ARP (RFC 826).
pub const ETHERTYPE_ARP: u16 = 0x0806;
/// IP protocol numbers (RFC 790).
pub const PROTO_ICMP: u8 = 1;
/// TCP.
pub const PROTO_TCP: u8 = 6;
/// UDP.
pub const PROTO_UDP: u8 = 17;

/// Bytes of an Ethernet header (no VLAN tag).
pub const ETH_HEADER: usize = 14;
/// Bytes of an IPv4 header without options.
pub const IP_HEADER: usize = 20;
/// Largest IP packet on Ethernet (MTU).
pub const MTU: usize = 1500;

pub(crate) fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

pub(crate) fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub(crate) fn ip_at(b: &[u8], at: usize) -> Ip {
    [b[at], b[at + 1], b[at + 2], b[at + 3]]
}

/// The Internet checksum (RFC 1071): the ones' complement of the ones'
/// complement sum of 16-bit words, `initial` being a partial sum (the
/// pseudo header).
pub fn checksum(data: &[u8], initial: u32) -> u16 {
    let mut sum = initial;
    let (words, rest) = data.as_chunks::<2>();
    for c in words {
        sum += u32::from(u16::from_be_bytes(*c));
    }
    if let [last] = rest {
        sum += u32::from(*last) << 8;
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// The partial sum of the TCP/UDP pseudo header (RFC 793 3.1, RFC 768).
pub fn pseudo_sum(src: Ip, dst: Ip, proto: u8, len: usize) -> u32 {
    u32::from(u16::from_be_bytes([src[0], src[1]]))
        + u32::from(u16::from_be_bytes([src[2], src[3]]))
        + u32::from(u16::from_be_bytes([dst[0], dst[1]]))
        + u32::from(u16::from_be_bytes([dst[2], dst[3]]))
        + u32::from(proto)
        + len as u32
}

/// A parsed Ethernet frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EthFrame<'a> {
    /// Destination.
    pub dst: Mac,
    /// Source.
    pub src: Mac,
    /// EtherType.
    pub ethertype: u16,
    /// What follows the header.
    pub payload: &'a [u8],
}

/// Parses an Ethernet frame (without the CRC, which the card strips).
pub fn parse_eth(frame: &[u8]) -> Option<EthFrame<'_>> {
    if frame.len() < ETH_HEADER {
        return None;
    }
    let mut dst = [0; 6];
    let mut src = [0; 6];
    dst.copy_from_slice(&frame[0..6]);
    src.copy_from_slice(&frame[6..12]);
    Some(EthFrame {
        dst,
        src,
        ethertype: be16(frame, 12),
        payload: &frame[ETH_HEADER..],
    })
}

/// An Ethernet frame around `payload`, padded to the 60-byte minimum.
pub fn build_eth(dst: Mac, src: Mac, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity((ETH_HEADER + payload.len()).max(60));
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    if f.len() < 60 {
        f.resize(60, 0);
    }
    f
}

/// ARP operation codes (RFC 826).
pub const ARP_REQUEST: u16 = 1;
/// Reply.
pub const ARP_REPLY: u16 = 2;

/// An ARP packet for IPv4 over Ethernet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arp {
    /// `ARP_REQUEST` or `ARP_REPLY`.
    pub op: u16,
    /// Sender hardware address.
    pub sender_mac: Mac,
    /// Sender protocol address.
    pub sender_ip: Ip,
    /// Target hardware address (zero in a request).
    pub target_mac: Mac,
    /// Target protocol address.
    pub target_ip: Ip,
}

/// Parses an ARP packet (hardware type 1, protocol IPv4, lengths 6 and 4).
pub fn parse_arp(p: &[u8]) -> Option<Arp> {
    if p.len() < 28 || be16(p, 0) != 1 || be16(p, 2) != ETHERTYPE_IPV4 || p[4] != 6 || p[5] != 4 {
        return None;
    }
    let mut sender_mac = [0; 6];
    let mut target_mac = [0; 6];
    sender_mac.copy_from_slice(&p[8..14]);
    target_mac.copy_from_slice(&p[18..24]);
    Some(Arp {
        op: be16(p, 6),
        sender_mac,
        sender_ip: ip_at(p, 14),
        target_mac,
        target_ip: ip_at(p, 24),
    })
}

/// The 28 bytes of an ARP packet.
pub fn build_arp(a: &Arp) -> Vec<u8> {
    let mut p = Vec::with_capacity(28);
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
    p.extend_from_slice(&[6, 4]);
    p.extend_from_slice(&a.op.to_be_bytes());
    p.extend_from_slice(&a.sender_mac);
    p.extend_from_slice(&a.sender_ip);
    p.extend_from_slice(&a.target_mac);
    p.extend_from_slice(&a.target_ip);
    p
}

/// A parsed IPv4 packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4<'a> {
    /// Source.
    pub src: Ip,
    /// Destination.
    pub dst: Ip,
    /// Protocol (`PROTO_*`).
    pub proto: u8,
    /// Time to live.
    pub ttl: u8,
    /// The payload (trimmed to the total length).
    pub payload: &'a [u8],
}

/// Parses an IPv4 packet: version 4, a valid header checksum, a total
/// length that fits; fragments (MF set or an offset) are refused (no
/// reassembly).
pub fn parse_ipv4(p: &[u8]) -> Option<Ipv4<'_>> {
    if p.len() < IP_HEADER || p[0] >> 4 != 4 {
        return None;
    }
    let ihl = usize::from(p[0] & 0xF) * 4;
    let total = usize::from(be16(p, 2));
    if ihl < IP_HEADER || total < ihl || total > p.len() || checksum(&p[..ihl], 0) != 0 {
        return None;
    }
    let frag = be16(p, 6);
    if frag & 0x2000 != 0 || frag & 0x1FFF != 0 {
        return None;
    }
    Some(Ipv4 {
        src: ip_at(p, 12),
        dst: ip_at(p, 16),
        proto: p[9],
        ttl: p[8],
        payload: &p[ihl..total],
    })
}

/// An IPv4 packet (no options, don't-fragment set, TTL 64).
pub fn build_ipv4(src: Ip, dst: Ip, proto: u8, id: u16, payload: &[u8]) -> Vec<u8> {
    let total = IP_HEADER + payload.len();
    let mut p = Vec::with_capacity(total);
    p.extend_from_slice(&[0x45, 0]);
    p.extend_from_slice(&(total as u16).to_be_bytes());
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&0x4000u16.to_be_bytes());
    p.extend_from_slice(&[64, proto, 0, 0]);
    p.extend_from_slice(&src);
    p.extend_from_slice(&dst);
    let sum = checksum(&p, 0);
    p[10..12].copy_from_slice(&sum.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// ICMP types (RFC 792).
pub const ICMP_ECHO_REPLY: u8 = 0;
/// Echo request.
pub const ICMP_ECHO_REQUEST: u8 = 8;

/// A parsed ICMP echo request or reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Echo<'a> {
    /// `ICMP_ECHO_REQUEST` or `ICMP_ECHO_REPLY`.
    pub kind: u8,
    /// Identifier.
    pub id: u16,
    /// Sequence number.
    pub seq: u16,
    /// The data.
    pub data: &'a [u8],
}

/// Parses an ICMP echo message (checksum checked).
pub fn parse_echo(p: &[u8]) -> Option<Echo<'_>> {
    if p.len() < 8 || checksum(p, 0) != 0 || p[1] != 0 {
        return None;
    }
    if p[0] != ICMP_ECHO_REQUEST && p[0] != ICMP_ECHO_REPLY {
        return None;
    }
    Some(Echo {
        kind: p[0],
        id: be16(p, 4),
        seq: be16(p, 6),
        data: &p[8..],
    })
}

/// An ICMP echo message.
pub fn build_echo(kind: u8, id: u16, seq: u16, data: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(8 + data.len());
    p.extend_from_slice(&[kind, 0, 0, 0]);
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(data);
    let sum = checksum(&p, 0);
    p[2..4].copy_from_slice(&sum.to_be_bytes());
    p
}

/// A parsed UDP datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Udp<'a> {
    /// Source port.
    pub src_port: u16,
    /// Destination port.
    pub dst_port: u16,
    /// The data.
    pub data: &'a [u8],
}

/// Parses a UDP datagram from `src` to `dst`; the checksum is checked
/// unless it is 0 (none).
pub fn parse_udp(src: Ip, dst: Ip, p: &[u8]) -> Option<Udp<'_>> {
    if p.len() < 8 {
        return None;
    }
    let len = usize::from(be16(p, 4));
    if len < 8 || len > p.len() {
        return None;
    }
    if be16(p, 6) != 0 && checksum(&p[..len], pseudo_sum(src, dst, PROTO_UDP, len)) != 0 {
        return None;
    }
    Some(Udp {
        src_port: be16(p, 0),
        dst_port: be16(p, 2),
        data: &p[8..len],
    })
}

/// A UDP datagram with its checksum.
pub fn build_udp(src: Ip, dst: Ip, src_port: u16, dst_port: u16, data: &[u8]) -> Vec<u8> {
    let len = 8 + data.len();
    let mut p = Vec::with_capacity(len);
    p.extend_from_slice(&src_port.to_be_bytes());
    p.extend_from_slice(&dst_port.to_be_bytes());
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(data);
    let mut sum = checksum(&p, pseudo_sum(src, dst, PROTO_UDP, len));
    if sum == 0 {
        sum = 0xFFFF;
    }
    p[6..8].copy_from_slice(&sum.to_be_bytes());
    p
}

/// TCP header flags (RFC 793 3.1).
pub mod tcp_flags {
    /// No more data from the sender.
    pub const FIN: u8 = 0x01;
    /// Synchronise sequence numbers.
    pub const SYN: u8 = 0x02;
    /// Reset the connection.
    pub const RST: u8 = 0x04;
    /// Push.
    pub const PSH: u8 = 0x08;
    /// The acknowledgment field is significant.
    pub const ACK: u8 = 0x10;
}

/// A parsed TCP segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tcp<'a> {
    /// Source port.
    pub src_port: u16,
    /// Destination port.
    pub dst_port: u16,
    /// Sequence number.
    pub seq: u32,
    /// Acknowledgment number.
    pub ack: u32,
    /// Flags (`tcp_flags`).
    pub flags: u8,
    /// Receive window.
    pub window: u16,
    /// Maximum segment size option (SYN only), if present.
    pub mss: Option<u16>,
    /// The data.
    pub data: &'a [u8],
}

/// Parses a TCP segment from `src` to `dst` (checksum checked).
pub fn parse_tcp(src: Ip, dst: Ip, p: &[u8]) -> Option<Tcp<'_>> {
    if p.len() < 20 || checksum(p, pseudo_sum(src, dst, PROTO_TCP, p.len())) != 0 {
        return None;
    }
    let offset = usize::from(p[12] >> 4) * 4;
    if offset < 20 || offset > p.len() {
        return None;
    }
    // Options: only MSS (kind 2) is read; End (0) stops, NOP (1) skips.
    let mut mss = None;
    let mut i = 20;
    while i < offset {
        match p[i] {
            0 => break,
            1 => i += 1,
            kind => {
                let Some(&len) = p.get(i + 1) else { break };
                let len = usize::from(len);
                if len < 2 || i + len > offset {
                    break;
                }
                if kind == 2 && len == 4 {
                    mss = Some(be16(p, i + 2));
                }
                i += len;
            }
        }
    }
    Some(Tcp {
        src_port: be16(p, 0),
        dst_port: be16(p, 2),
        seq: be32(p, 4),
        ack: be32(p, 8),
        flags: p[13],
        window: be16(p, 14),
        mss,
        data: &p[offset..],
    })
}

/// The fields of a TCP segment to build.
#[derive(Clone, Copy, Debug)]
pub struct TcpOut<'a> {
    /// Source port.
    pub src_port: u16,
    /// Destination port.
    pub dst_port: u16,
    /// Sequence number.
    pub seq: u32,
    /// Acknowledgment number.
    pub ack: u32,
    /// Flags.
    pub flags: u8,
    /// Receive window.
    pub window: u16,
    /// MSS option to include (SYN segments).
    pub mss: Option<u16>,
    /// The data.
    pub data: &'a [u8],
}

/// A TCP segment with its checksum.
pub fn build_tcp(src: Ip, dst: Ip, s: &TcpOut) -> Vec<u8> {
    let header = if s.mss.is_some() { 24 } else { 20 };
    let len = header + s.data.len();
    let mut p = Vec::with_capacity(len);
    p.extend_from_slice(&s.src_port.to_be_bytes());
    p.extend_from_slice(&s.dst_port.to_be_bytes());
    p.extend_from_slice(&s.seq.to_be_bytes());
    p.extend_from_slice(&s.ack.to_be_bytes());
    p.push(((header / 4) as u8) << 4);
    p.push(s.flags);
    p.extend_from_slice(&s.window.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]);
    if let Some(mss) = s.mss {
        p.extend_from_slice(&[2, 4]);
        p.extend_from_slice(&mss.to_be_bytes());
    }
    p.extend_from_slice(s.data);
    let sum = checksum(&p, pseudo_sum(src, dst, PROTO_TCP, len));
    p[16..18].copy_from_slice(&sum.to_be_bytes());
    p
}

/// "a.b.c.d" to an address.
pub fn parse_ip(text: &str) -> Option<Ip> {
    let mut ip = [0u8; 4];
    let mut parts = text.split('.');
    for b in &mut ip {
        *b = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_rfc1071_example() {
        // RFC 1071 section 3: 00 01 f2 03 f4 f5 f6 f7 sums to ddf2.
        let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(checksum(&data, 0), !0xddf2);
        // Odd length: the last byte is the high half of a word.
        assert_eq!(checksum(&[0x12], 0), !0x1200);
    }

    #[test]
    fn ethernet_round_trip_and_padding() {
        let f = build_eth(BROADCAST_MAC, [1, 2, 3, 4, 5, 6], ETHERTYPE_ARP, &[9; 10]);
        assert_eq!(f.len(), 60);
        let e = parse_eth(&f).expect("frame");
        assert_eq!(e.dst, BROADCAST_MAC);
        assert_eq!(e.src, [1, 2, 3, 4, 5, 6]);
        assert_eq!(e.ethertype, ETHERTYPE_ARP);
        assert_eq!(&e.payload[..10], &[9; 10]);
        assert!(parse_eth(&[0; 13]).is_none());
    }

    #[test]
    fn arp_round_trip() {
        let a = Arp {
            op: ARP_REQUEST,
            sender_mac: [0x52, 0x54, 0, 0x12, 0x34, 0x56],
            sender_ip: [10, 0, 2, 15],
            target_mac: [0; 6],
            target_ip: [10, 0, 2, 2],
        };
        let p = build_arp(&a);
        assert_eq!(p.len(), 28);
        assert_eq!(parse_arp(&p), Some(a));
        let mut bad = p.clone();
        bad[4] = 8;
        assert_eq!(parse_arp(&bad), None);
    }

    #[test]
    fn ipv4_round_trip_and_checks() {
        let p = build_ipv4([10, 0, 2, 15], [10, 0, 2, 2], PROTO_UDP, 7, b"hello");
        assert_eq!(checksum(&p[..20], 0), 0);
        let ip = parse_ipv4(&p).expect("packet");
        assert_eq!(ip.src, [10, 0, 2, 15]);
        assert_eq!(ip.dst, [10, 0, 2, 2]);
        assert_eq!(ip.proto, PROTO_UDP);
        assert_eq!(ip.payload, b"hello");
        // Ethernet padding after the packet is ignored.
        let mut padded = p.clone();
        padded.extend_from_slice(&[0; 20]);
        assert_eq!(parse_ipv4(&padded).expect("padded").payload, b"hello");
        let mut bad = p.clone();
        bad[8] = 1; // TTL changed: checksum fails
        assert!(parse_ipv4(&bad).is_none());
        let mut frag = p.clone();
        frag[6] = 0x20; // MF
        frag[10] = 0;
        frag[11] = 0;
        let sum = checksum(&frag[..20], 0);
        frag[10..12].copy_from_slice(&sum.to_be_bytes());
        assert!(parse_ipv4(&frag).is_none());
    }

    #[test]
    fn icmp_echo_round_trip() {
        let p = build_echo(ICMP_ECHO_REQUEST, 0x1234, 7, b"ping");
        let e = parse_echo(&p).expect("echo");
        assert_eq!(
            (e.kind, e.id, e.seq, e.data),
            (ICMP_ECHO_REQUEST, 0x1234, 7, &b"ping"[..])
        );
        let mut bad = p.clone();
        bad[9] ^= 1;
        assert!(parse_echo(&bad).is_none());
    }

    #[test]
    fn udp_round_trip_and_checksum() {
        let (a, b) = ([10, 0, 2, 15], [10, 0, 2, 3]);
        let p = build_udp(a, b, 5000, 53, b"query");
        let u = parse_udp(a, b, &p).expect("datagram");
        assert_eq!((u.src_port, u.dst_port, u.data), (5000, 53, &b"query"[..]));
        // A different pseudo header fails the checksum.
        assert!(parse_udp(a, [10, 0, 2, 4], &p).is_none());
        // Checksum 0 means none.
        let mut none = p.clone();
        none[6] = 0;
        none[7] = 0;
        assert!(parse_udp(a, [10, 0, 2, 4], &none).is_some());
    }

    #[test]
    fn tcp_round_trip_with_mss() {
        let (a, b) = ([10, 0, 2, 15], [10, 0, 2, 2]);
        let p = build_tcp(
            a,
            b,
            &TcpOut {
                src_port: 49152,
                dst_port: 80,
                seq: 1000,
                ack: 0,
                flags: tcp_flags::SYN,
                window: 65535,
                mss: Some(1460),
                data: &[],
            },
        );
        assert_eq!(p.len(), 24);
        let t = parse_tcp(a, b, &p).expect("segment");
        assert_eq!(
            (t.src_port, t.dst_port, t.seq, t.flags),
            (49152, 80, 1000, tcp_flags::SYN)
        );
        assert_eq!(t.mss, Some(1460));
        assert_eq!(t.window, 65535);
        let d = build_tcp(
            a,
            b,
            &TcpOut {
                src_port: 1,
                dst_port: 2,
                seq: 5,
                ack: 6,
                flags: tcp_flags::ACK | tcp_flags::PSH,
                window: 100,
                mss: None,
                data: b"GET /",
            },
        );
        let t = parse_tcp(a, b, &d).expect("data segment");
        assert_eq!((t.ack, t.data, t.mss), (6, &b"GET /"[..], None));
        let mut bad = d.clone();
        bad[24] ^= 0x20;
        assert!(parse_tcp(a, b, &bad).is_none());
    }

    #[test]
    fn ip_text() {
        assert_eq!(parse_ip("10.0.2.15"), Some([10, 0, 2, 15]));
        assert_eq!(parse_ip("10.0.2"), None);
        assert_eq!(parse_ip("10.0.2.256"), None);
        assert_eq!(parse_ip("1.2.3.4.5"), None);
    }
}
