// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Networking (`docs/net.md`): sockets over the kernel's stack, DNS name
//! lookups (A records, over UDP to the DHCP-given server) and a small
//! HTTP/1.0 GET.

use crate::sys::{syscall1, syscall2, syscall3, syscall4, syscall6};
use abi::net::{self as n, NetInfo};
use abi::{Error, Handle, Syscall};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// An IPv4 address.
pub type Ip = [u8; 4];

/// Wait forever.
pub const FOREVER: u64 = u64::MAX;

/// A socket; the handle is closed when it is dropped.
pub struct Socket(Handle);

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = crate::ipc::handle_close(self.0);
    }
}

impl Socket {
    /// Needs the `NETWORK` capability (`AccessDenied` without).
    fn create(kind: u64) -> Result<Socket, Error> {
        let network = crate::process::capability(abi::capability::NETWORK).unwrap_or(Handle::NONE);
        syscall2(Syscall::SocketCreate, kind, network.raw()).map(|h| Socket(Handle::from_raw(h)))
    }

    /// A UDP socket.
    pub fn udp() -> Result<Socket, Error> {
        Socket::create(n::UDP)
    }

    /// A TCP socket.
    pub fn tcp() -> Result<Socket, Error> {
        Socket::create(n::TCP)
    }

    /// An ICMP echo socket.
    pub fn icmp() -> Result<Socket, Error> {
        Socket::create(n::ICMP)
    }

    /// The handle (stays owned by the socket).
    pub fn handle(&self) -> Handle {
        self.0
    }

    /// Takes the handle out (it is no longer closed on drop).
    pub fn into_handle(self) -> Handle {
        let h = self.0;
        core::mem::forget(self);
        h
    }

    /// Binds local port `port` (0: any); returns the port.
    pub fn bind(&self, port: u16) -> Result<u16, Error> {
        syscall2(Syscall::SocketBind, self.0.raw(), u64::from(port)).map(|p| p as u16)
    }

    /// TCP: connects (waiting up to `timeout_ns`); UDP: sets the default
    /// destination.
    pub fn connect(&self, ip: Ip, port: u16, timeout_ns: u64) -> Result<(), Error> {
        syscall3(
            Syscall::SocketConnect,
            self.0.raw(),
            n::pack(ip, port),
            timeout_ns,
        )
        .map(|_| ())
    }

    /// TCP: listens on the bound port.
    pub fn listen(&self, backlog: usize) -> Result<(), Error> {
        syscall2(Syscall::SocketListen, self.0.raw(), backlog as u64).map(|_| ())
    }

    /// TCP: the next connection.
    pub fn accept(&self, timeout_ns: u64) -> Result<Socket, Error> {
        syscall2(Syscall::SocketAccept, self.0.raw(), timeout_ns)
            .map(|h| Socket(Handle::from_raw(h)))
    }

    /// Sends `data` (TCP: as much as fits, waiting for room; UDP: one
    /// datagram to `to` or the connected peer; ICMP: an echo request to
    /// `to.0` with sequence number `to.1`).
    pub fn send(&self, data: &[u8], to: Option<(Ip, u16)>) -> Result<usize, Error> {
        let addr = to.map_or(0, |(ip, port)| n::pack(ip, port));
        syscall4(
            Syscall::SocketSend,
            self.0.raw(),
            data.as_ptr() as u64,
            data.len() as u64,
            addr,
        )
        .map(|v| v as usize)
    }

    /// TCP: sends all of `data`.
    pub fn send_all(&self, mut data: &[u8]) -> Result<(), Error> {
        while !data.is_empty() {
            let n = self.send(data, None)?;
            data = &data[n..];
        }
        Ok(())
    }

    /// Receives into `buf`, waiting up to `timeout_ns`: the byte count
    /// (TCP: 0 at the end of the stream) and the source.
    pub fn recv(&self, buf: &mut [u8], timeout_ns: u64) -> Result<(usize, Ip, u16), Error> {
        let mut addr = [0u8; 8];
        let len = syscall6(
            Syscall::SocketRecv,
            [
                self.0.raw(),
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                timeout_ns,
                addr.as_mut_ptr() as u64,
                0,
            ],
        )? as usize;
        let (ip, port) = n::unpack(u64::from_le_bytes(addr));
        Ok((len, ip, port))
    }
}

/// The network card and address; `None` without a card.
pub fn info() -> Option<NetInfo> {
    let mut b = [0u8; n::NET_INFO_SIZE];
    match syscall1(Syscall::NetInfo, b.as_mut_ptr() as u64) {
        Ok(1) => NetInfo::from_bytes(&b),
        _ => None,
    }
}

/// Waits up to `timeout_ns` for the interface to have an address.
pub fn wait_configured(timeout_ns: u64) -> Option<NetInfo> {
    let end = crate::now_ns().saturating_add(timeout_ns);
    loop {
        let i = info()?;
        if i.configured {
            return Some(i);
        }
        if crate::now_ns() >= end {
            return None;
        }
        crate::sleep_ms(50);
    }
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

/// An address as text.
pub fn ip_text(ip: Ip) -> String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// A DNS query for the A record of `name` (RFC 1035 4.1): header with
/// id `id` and recursion desired, one question.
pub fn dns_query(id: u16, name: &str) -> Option<Vec<u8>> {
    let mut q = Vec::with_capacity(32 + name.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&[0, 1, 0, 1]); // type A, class IN
    Some(q)
}

/// Skips a (possibly compressed) name at `at`; returns the offset after it.
fn skip_name(p: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = *p.get(at)?;
        if len & 0xC0 == 0xC0 {
            return Some(at + 2);
        }
        if len == 0 {
            return Some(at + 1);
        }
        at += 1 + usize::from(len);
    }
}

/// The first A record of a DNS response to query `id`.
pub fn dns_answer(p: &[u8], id: u16) -> Option<Ip> {
    if p.len() < 12 || u16::from_be_bytes([p[0], p[1]]) != id || p[2] & 0x80 == 0 {
        return None;
    }
    if p[3] & 0x0F != 0 {
        return None; // an error code (NXDOMAIN...)
    }
    let qd = u16::from_be_bytes([p[4], p[5]]);
    let an = u16::from_be_bytes([p[6], p[7]]);
    let mut at = 12;
    for _ in 0..qd {
        at = skip_name(p, at)? + 4;
    }
    for _ in 0..an {
        at = skip_name(p, at)?;
        let rr = p.get(at..at + 10)?;
        let kind = u16::from_be_bytes([rr[0], rr[1]]);
        let len = usize::from(u16::from_be_bytes([rr[8], rr[9]]));
        let data = p.get(at + 10..at + 10 + len)?;
        if kind == 1 && len == 4 {
            return Some([data[0], data[1], data[2], data[3]]);
        }
        at += 10 + len;
    }
    None
}

/// The address of `host`: an "a.b.c.d" literal, or a DNS lookup (3 tries
/// of 2 seconds).
pub fn resolve(host: &str) -> Result<Ip, Error> {
    if let Some(ip) = parse_ip(host) {
        return Ok(ip);
    }
    let i = info().ok_or(Error::NetworkDown)?;
    if !i.configured || i.dns == [0; 4] {
        return Err(Error::NetworkDown);
    }
    let id = crate::now_ns() as u16 | 1;
    let query = dns_query(id, host).ok_or(Error::InvalidArgument)?;
    let s = Socket::udp()?;
    let mut buf = [0u8; 512];
    for _ in 0..3 {
        s.send(&query, Some((i.dns, 53)))?;
        let end = crate::now_ns() + 2_000_000_000;
        loop {
            let left = end.saturating_sub(crate::now_ns());
            match s.recv(&mut buf, left) {
                Ok((len, _, 53)) => {
                    if let Some(ip) = dns_answer(&buf[..len], id) {
                        return Ok(ip);
                    }
                    if buf.len() >= 4 && len >= 4 && buf[3] & 0x0F == 3 {
                        return Err(Error::NotFound);
                    }
                }
                Ok(_) => {}
                Err(Error::TimedOut) => break,
                Err(e) => return Err(e),
            }
        }
    }
    Err(Error::TimedOut)
}

/// The parts of an `http://host[:port]/path` URL.
pub fn parse_url(url: &str) -> Option<(&str, u16, &str)> {
    match parse_any_url(url)? {
        (false, host, port, path) => Some((host, port, path)),
        (true, ..) => None,
    }
}

/// The parts of an `http://` or `https://host[:port]/path` URL: (TLS,
/// host, port (default 80 or 443), path).
pub fn parse_any_url(url: &str) -> Option<(bool, &str, u16, &str)> {
    let (secure, rest) = match url.strip_prefix("https://") {
        Some(rest) => (true, rest),
        None => (false, url.strip_prefix("http://")?),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let default = if secure { 443 } else { 80 };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (authority, default),
    };
    (!host.is_empty()).then_some((secure, host, port, path))
}

/// An HTTP response: the status code and the body.
pub struct Response {
    /// The status code (200...).
    pub status: u16,
    /// The body.
    pub body: Vec<u8>,
}

/// Splits a raw HTTP response into status and body.
pub fn parse_response(raw: &[u8]) -> Option<Response> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = core::str::from_utf8(&raw[..end]).ok()?;
    let status_line = head.lines().next()?;
    let mut parts = status_line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let status = parts.next()?.parse().ok()?;
    Some(Response {
        status,
        body: raw[end + 4..].to_vec(),
    })
}

/// `GET url` over HTTP/1.0 (the server closes the connection at the
/// end); `timeout_ns` for the connection and each read.
pub fn http_get(url: &str, timeout_ns: u64) -> Result<Response, Error> {
    parse_response(&http_get_raw(url, timeout_ns)?).ok_or(Error::IoError)
}

/// The whole raw response (head and body) to `GET url` (`crate::http`
/// makes sense of it). `https://` (P12) trusts the system's roots.
pub fn http_get_raw(url: &str, timeout_ns: u64) -> Result<Vec<u8>, Error> {
    http_get_raw_with(url, timeout_ns, None)
}

/// [`http_get_raw`] trusting `roots` for `https://` (`None`: the
/// system's, `Initrd:/certs/cacert.der`).
pub fn http_get_raw_with(
    url: &str,
    timeout_ns: u64,
    roots: Option<&crate::tls::Roots>,
) -> Result<Vec<u8>, Error> {
    let (secure, host, port, path) = parse_any_url(url).ok_or(Error::InvalidArgument)?;
    let ip = resolve(host)?;
    let request = alloc::format!(
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: STELA\r\nConnection: close\r\n\r\n"
    );
    if secure {
        #[cfg(target_os = "none")]
        {
            let system;
            let roots = match roots {
                Some(r) => r,
                None => {
                    system = crate::tls::Roots::system()?;
                    &system
                }
            };
            return crate::tls::request(host, ip, port, request.as_bytes(), roots, timeout_ns);
        }
        #[cfg(not(target_os = "none"))]
        {
            let _ = roots;
            return Err(Error::NotSupported);
        }
    }
    let s = Socket::tcp()?;
    s.connect(ip, port, timeout_ns)?;
    s.send_all(request.as_bytes())?;
    let mut raw = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let (n, ..) = s.recv(&mut buf, timeout_ns)?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_query_layout() {
        let q = dns_query(0x1234, "example.com").expect("query");
        assert_eq!(&q[0..4], &[0x12, 0x34, 0x01, 0x00]);
        assert_eq!(&q[12..25], b"\x07example\x03com\x00");
        assert_eq!(&q[25..29], &[0, 1, 0, 1]);
        assert!(dns_query(1, "a..b").is_none());
    }

    #[test]
    fn dns_answer_with_compression() {
        let mut r = dns_query(7, "stela.test").expect("query");
        r[2] = 0x81;
        r[3] = 0x80;
        r[7] = 2; // two answers: a CNAME, then the A record
        // CNAME: pointer to the question name, type 5, class 1, TTL, 2 bytes.
        r.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2, 0xC0, 12]);
        // A: pointer, type 1, class 1, TTL, 4 bytes.
        r.extend_from_slice(&[0xC0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 216, 34]);
        assert_eq!(dns_answer(&r, 7), Some([93, 184, 216, 34]));
        assert_eq!(dns_answer(&r, 8), None);
        r[3] = 0x83; // NXDOMAIN
        assert_eq!(dns_answer(&r, 7), None);
    }

    #[test]
    fn urls() {
        assert_eq!(
            parse_url("http://10.0.2.2:8000/hello.txt"),
            Some(("10.0.2.2", 8000, "/hello.txt"))
        );
        assert_eq!(
            parse_url("http://example.com"),
            Some(("example.com", 80, "/"))
        );
        assert_eq!(parse_url("https://example.com/"), None);
        assert_eq!(parse_url("http://:80/"), None);
    }

    #[test]
    fn responses() {
        let r = parse_response(b"HTTP/1.0 200 OK\r\nServer: x\r\n\r\nbody").expect("response");
        assert_eq!((r.status, r.body.as_slice()), (200, &b"body"[..]));
        assert!(parse_response(b"garbage").is_none());
    }

    #[test]
    fn ip_text_round_trip() {
        assert_eq!(parse_ip("10.0.2.15"), Some([10, 0, 2, 15]));
        assert_eq!(ip_text([10, 0, 2, 15]), "10.0.2.15");
        assert_eq!(parse_ip("10.0.2"), None);
    }
}
