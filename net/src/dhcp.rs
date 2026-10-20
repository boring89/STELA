// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! DHCP messages (RFC 2131, options RFC 2132): the client side's
//! DISCOVER and REQUEST, and the server's OFFER and ACK/NAK.
//!
//! ```text
//!  client                         server
//!   DISCOVER (broadcast) ───────►
//!            ◄───────────────── OFFER   (your IP, server id, mask, router, DNS)
//!   REQUEST  (broadcast) ───────►       (requested IP, server id)
//!            ◄───────────────── ACK     (the same options, the lease)
//! ```

use crate::wire::{Ip, Mac, be32, ip_at};
use alloc::vec::Vec;

/// UDP port of the server.
pub const SERVER_PORT: u16 = 67;
/// UDP port of the client.
pub const CLIENT_PORT: u16 = 68;

/// Message types (option 53).
pub const DISCOVER: u8 = 1;
/// OFFER.
pub const OFFER: u8 = 2;
/// REQUEST.
pub const REQUEST: u8 = 3;
/// ACK.
pub const ACK: u8 = 5;
/// NAK.
pub const NAK: u8 = 6;

const MAGIC: u32 = 0x6382_5363;

/// What a server's message says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// Message type (option 53).
    pub kind: u8,
    /// Transaction id.
    pub xid: u32,
    /// The address offered ("yiaddr").
    pub your_ip: Ip,
    /// Server identifier (option 54).
    pub server: Ip,
    /// Subnet mask (option 1).
    pub mask: Ip,
    /// First router (option 3).
    pub router: Ip,
    /// First DNS server (option 6).
    pub dns: Ip,
    /// Lease time in seconds (option 51).
    pub lease: u32,
}

/// A DISCOVER (`requested` and `server` absent) or REQUEST message from
/// the client with hardware address `mac`.
pub fn build(kind: u8, xid: u32, mac: Mac, requested: Option<Ip>, server: Option<Ip>) -> Vec<u8> {
    let mut p = Vec::with_capacity(300);
    // op 1 (request), htype 1 (Ethernet), hlen 6, hops 0.
    p.extend_from_slice(&[1, 1, 6, 0]);
    p.extend_from_slice(&xid.to_be_bytes());
    p.extend_from_slice(&[0, 0]); // secs
    p.extend_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast replies
    p.extend_from_slice(&[0; 16]); // ciaddr, yiaddr, siaddr, giaddr
    p.extend_from_slice(&mac);
    p.extend_from_slice(&[0; 10]); // chaddr padding
    p.extend_from_slice(&[0; 192]); // sname, file
    p.extend_from_slice(&MAGIC.to_be_bytes());
    p.extend_from_slice(&[53, 1, kind]);
    if let Some(ip) = requested {
        p.extend_from_slice(&[50, 4]);
        p.extend_from_slice(&ip);
    }
    if let Some(ip) = server {
        p.extend_from_slice(&[54, 4]);
        p.extend_from_slice(&ip);
    }
    // Parameter request list: mask, router, DNS.
    p.extend_from_slice(&[55, 3, 1, 3, 6]);
    p.push(255);
    // Pad to the 300 bytes of a BOOTP message (RFC 951) some servers want.
    if p.len() < 300 {
        p.resize(300, 0);
    }
    p
}

/// Parses a server's reply (op 2, Ethernet, the magic cookie, a message
/// type) for the client `mac`.
pub fn parse(p: &[u8], mac: Mac) -> Option<Reply> {
    if p.len() < 240 || p[0] != 2 || p[1] != 1 || p[2] != 6 || p[28..34] != mac {
        return None;
    }
    if be32(p, 236) != MAGIC {
        return None;
    }
    let mut r = Reply {
        xid: be32(p, 4),
        your_ip: ip_at(p, 16),
        ..Reply::default()
    };
    let mut i = 240;
    while i < p.len() {
        let code = p[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        let len = usize::from(*p.get(i + 1)?);
        let v = p.get(i + 2..i + 2 + len)?;
        match (code, len) {
            (53, 1) => r.kind = v[0],
            (1, 4) => r.mask = ip_at(v, 0),
            (3, l) if l >= 4 => r.router = ip_at(v, 0),
            (6, l) if l >= 4 => r.dns = ip_at(v, 0),
            (51, 4) => r.lease = be32(v, 0),
            (54, 4) => r.server = ip_at(v, 0),
            _ => {}
        }
        i += 2 + len;
    }
    (r.kind != 0).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: Mac = [0x52, 0x54, 0, 0x12, 0x34, 0x56];

    /// A server reply as slirp sends it.
    fn reply(kind: u8, xid: u32) -> Vec<u8> {
        let mut p = build(kind, xid, MAC, None, None);
        p[0] = 2;
        p[16..20].copy_from_slice(&[10, 0, 2, 15]);
        let mut opts = alloc::vec![53, 1, kind, 54, 4, 10, 0, 2, 2, 1, 4, 255, 255, 255, 0];
        opts.extend_from_slice(&[
            3, 4, 10, 0, 2, 2, 6, 4, 10, 0, 2, 3, 51, 4, 0, 1, 0x51, 0x80, 255,
        ]);
        p.truncate(240);
        p.extend_from_slice(&opts);
        p
    }

    #[test]
    fn discover_layout() {
        let p = build(DISCOVER, 0xABCD_0123, MAC, None, None);
        assert_eq!(p.len(), 300);
        assert_eq!(&p[0..4], &[1, 1, 6, 0]);
        assert_eq!(be32(&p, 4), 0xABCD_0123);
        assert_eq!(&p[28..34], &MAC);
        assert_eq!(be32(&p, 236), MAGIC);
        assert_eq!(&p[240..243], &[53, 1, DISCOVER]);
        let r = build(REQUEST, 1, MAC, Some([10, 0, 2, 15]), Some([10, 0, 2, 2]));
        assert_eq!(&r[243..249], &[50, 4, 10, 0, 2, 15]);
        assert_eq!(&r[249..255], &[54, 4, 10, 0, 2, 2]);
    }

    #[test]
    fn offer_is_parsed() {
        let r = parse(&reply(OFFER, 77), MAC).expect("offer");
        assert_eq!(r.kind, OFFER);
        assert_eq!(r.xid, 77);
        assert_eq!(r.your_ip, [10, 0, 2, 15]);
        assert_eq!(r.server, [10, 0, 2, 2]);
        assert_eq!(r.mask, [255, 255, 255, 0]);
        assert_eq!(r.router, [10, 0, 2, 2]);
        assert_eq!(r.dns, [10, 0, 2, 3]);
        assert_eq!(r.lease, 86400);
    }

    #[test]
    fn others_are_refused() {
        // Another client's reply.
        assert!(parse(&reply(OFFER, 1), [1; 6]).is_none());
        // A request (op 1).
        assert!(parse(&build(DISCOVER, 1, MAC, None, None), MAC).is_none());
        // Truncated options.
        let mut p = reply(ACK, 1);
        p.truncate(250);
        assert!(parse(&p, MAC).is_none() || parse(&p, MAC).is_some_and(|r| r.kind == ACK));
    }
}
