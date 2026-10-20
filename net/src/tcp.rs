// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! One TCP connection (RFC 793): the transmission control block and its
//! state machine, without I/O. Incoming segments go in through
//! [`Tcb::on_segment`]; the segments to send pile up in [`Tcb::out`].
//!
//! ```text
//!  active open:  SynSent ──SYN+ACK──► Established
//!  passive open: (listener got SYN) SynReceived ──ACK──► Established
//!  we close first:  Established ─FIN─► FinWait1 ─ACK─► FinWait2 ─FIN─► TimeWait ─2 s─► Closed
//!  they close first: Established ─FIN─► CloseWait ─(close) FIN─► LastAck ─ACK─► Closed
//! ```
//!
//! Simplifications: segments that arrive out of order are dropped (and a
//! duplicate ACK sent), so the peer retransmits them; retransmission is
//! go-back-N from the oldest unacknowledged byte after a timeout of 1 s,
//! doubling to 8 s, 8 tries at most; every data segment is acknowledged
//! at once; the receive window is the free part of a 64 KiB buffer.

use crate::wire::{Ip, tcp_flags};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Bytes each direction buffers.
pub const BUFFER: usize = 64 * 1024;
/// Our maximum segment size (Ethernet MTU - IP and TCP headers).
pub const MSS: u16 = 1460;
/// The peer's MSS when it sends none (RFC 1122 4.2.2.6).
const DEFAULT_MSS: u16 = 536;
const RTO_MIN_MS: u64 = 1000;
const RTO_MAX_MS: u64 = 8000;
const MAX_RETRIES: u32 = 8;
/// TIME-WAIT is shortened to 2 seconds (RFC 793 asks for 2 MSL).
const TIME_WAIT_MS: u64 = 2000;

/// Connection states (RFC 793 3.2; LISTEN belongs to the listener).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// SYN sent, waiting for SYN+ACK.
    SynSent,
    /// SYN received and SYN+ACK sent, waiting for the ACK.
    SynReceived,
    /// Open in both directions.
    Established,
    /// Our FIN sent, not yet acknowledged.
    FinWait1,
    /// Our FIN acknowledged, waiting for theirs.
    FinWait2,
    /// Their FIN received; we may still send.
    CloseWait,
    /// Both FINs sent at once; waiting for the ACK of ours.
    Closing,
    /// Their FIN, then ours; waiting for the ACK of ours.
    LastAck,
    /// Both sides done; lingering for stray segments.
    TimeWait,
    /// Gone (or never opened).
    Closed,
}

/// Why a connection ended badly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpError {
    /// The peer answered the SYN with RST (nothing listens there).
    Refused,
    /// The peer reset an open connection.
    Reset,
    /// No answer after every retransmission.
    TimedOut,
}

/// A segment to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Sequence number.
    pub seq: u32,
    /// Acknowledgment number.
    pub ack: u32,
    /// Flags.
    pub flags: u8,
    /// Our receive window.
    pub window: u16,
    /// MSS option (SYN segments).
    pub mss: Option<u16>,
    /// The data.
    pub data: Vec<u8>,
}

/// A received segment, as the stack hands it on.
#[derive(Clone, Copy, Debug)]
pub struct Incoming<'a> {
    /// Sequence number.
    pub seq: u32,
    /// Acknowledgment number.
    pub ack: u32,
    /// Flags.
    pub flags: u8,
    /// The peer's window.
    pub window: u16,
    /// The peer's MSS option.
    pub mss: Option<u16>,
    /// The data.
    pub data: &'a [u8],
}

/// `a < b` in sequence space (RFC 793 3.3, modulo 2^32).
fn lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

fn le(a: u32, b: u32) -> bool {
    a == b || lt(a, b)
}

/// The transmission control block.
#[derive(Debug)]
pub struct Tcb {
    /// The state.
    pub state: State,
    /// Our port.
    pub local_port: u16,
    /// The peer.
    pub remote_ip: Ip,
    /// The peer's port.
    pub remote_port: u16,
    /// Why it failed, if it did.
    pub error: Option<TcpError>,
    /// Segments waiting to be sent.
    pub out: Vec<Segment>,
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    snd_wnd: u32,
    /// Bytes from `snd_una` on: unacknowledged, then not yet sent.
    send_buf: VecDeque<u8>,
    /// The application closed: send FIN after the data.
    fin_queued: bool,
    /// The sequence number of our FIN, once sent.
    fin_seq: Option<u32>,
    rcv_nxt: u32,
    recv_buf: VecDeque<u8>,
    /// The peer sent FIN (end of its data).
    pub peer_fin: bool,
    mss: u16,
    rto_ms: u64,
    retransmit_at: Option<u64>,
    retries: u32,
    time_wait_until: u64,
}

impl Tcb {
    fn new(local_port: u16, remote_ip: Ip, remote_port: u16, iss: u32) -> Tcb {
        Tcb {
            state: State::Closed,
            local_port,
            remote_ip,
            remote_port,
            error: None,
            out: Vec::new(),
            iss,
            snd_una: iss,
            snd_nxt: iss,
            snd_wnd: 0,
            send_buf: VecDeque::new(),
            fin_queued: false,
            fin_seq: None,
            rcv_nxt: 0,
            recv_buf: VecDeque::new(),
            peer_fin: false,
            mss: DEFAULT_MSS,
            rto_ms: RTO_MIN_MS,
            retransmit_at: None,
            retries: 0,
            time_wait_until: 0,
        }
    }

    /// An active open: sends SYN with initial sequence number `iss`.
    pub fn connect(local_port: u16, remote_ip: Ip, remote_port: u16, iss: u32, now: u64) -> Tcb {
        let mut t = Tcb::new(local_port, remote_ip, remote_port, iss);
        t.state = State::SynSent;
        t.send_syn(now);
        t
    }

    /// A passive open: the listener on `local_port` got `syn`; answers
    /// with SYN+ACK.
    pub fn accept(
        local_port: u16,
        remote_ip: Ip,
        remote_port: u16,
        syn: &Incoming,
        iss: u32,
        now: u64,
    ) -> Tcb {
        let mut t = Tcb::new(local_port, remote_ip, remote_port, iss);
        t.state = State::SynReceived;
        t.rcv_nxt = syn.seq.wrapping_add(1);
        t.snd_wnd = u32::from(syn.window);
        t.mss = syn.mss.unwrap_or(DEFAULT_MSS).min(MSS);
        t.send_syn(now);
        t
    }

    /// Our receive window: the free part of the receive buffer.
    fn window(&self) -> u16 {
        (BUFFER - self.recv_buf.len()).min(0xFFFF) as u16
    }

    fn segment(&mut self, seq: u32, flags: u8, data: Vec<u8>) {
        let ack = if flags & tcp_flags::ACK != 0 {
            self.rcv_nxt
        } else {
            0
        };
        let mss = (flags & tcp_flags::SYN != 0).then_some(MSS);
        self.out.push(Segment {
            seq,
            ack,
            flags,
            window: self.window(),
            mss,
            data,
        });
    }

    fn send_syn(&mut self, now: u64) {
        let flags = if self.state == State::SynReceived {
            tcp_flags::SYN | tcp_flags::ACK
        } else {
            tcp_flags::SYN
        };
        self.segment(self.iss, flags, Vec::new());
        self.snd_nxt = self.iss.wrapping_add(1);
        self.retransmit_at = Some(now + self.rto_ms);
    }

    fn send_ack(&mut self) {
        self.segment(self.snd_nxt, tcp_flags::ACK, Vec::new());
    }

    /// Bytes the application may read now.
    pub fn readable(&self) -> usize {
        self.recv_buf.len()
    }

    /// Whether `recv` would report the end of the peer's data.
    pub fn at_end(&self) -> bool {
        self.recv_buf.is_empty() && (self.peer_fin || self.error.is_some())
    }

    /// Whether data may still be queued.
    pub fn can_send(&self) -> bool {
        matches!(self.state, State::Established | State::CloseWait) && !self.fin_queued
    }

    /// Room in the send buffer.
    pub fn send_room(&self) -> usize {
        BUFFER - self.send_buf.len()
    }

    /// Queues as much of `data` as fits; returns how much.
    pub fn send(&mut self, data: &[u8], now: u64) -> usize {
        if !self.can_send() {
            return 0;
        }
        let n = data.len().min(self.send_room());
        self.send_buf.extend(&data[..n]);
        self.output(now);
        n
    }

    /// Reads up to `buf.len()` received bytes.
    pub fn recv(&mut self, buf: &mut [u8]) -> usize {
        let before = self.window();
        let n = buf.len().min(self.recv_buf.len());
        for (b, v) in buf.iter_mut().zip(self.recv_buf.drain(..n)) {
            *b = v;
        }
        // Tell the peer when the window opens up again (it may have
        // stopped sending).
        if n > 0 && before < MSS && self.window() >= MSS && self.synchronised() {
            self.send_ack();
        }
        n
    }

    fn synchronised(&self) -> bool {
        !matches!(
            self.state,
            State::SynSent | State::SynReceived | State::Closed
        )
    }

    /// The application closes its side: FIN after the queued data.
    pub fn close(&mut self, now: u64) {
        match self.state {
            State::SynSent | State::Closed => self.state = State::Closed,
            State::SynReceived | State::Established | State::CloseWait => {
                self.fin_queued = true;
                self.output(now);
            }
            _ => {}
        }
    }

    /// Resets the connection (RST to the peer).
    pub fn abort(&mut self) {
        if self.synchronised() || self.state == State::SynReceived {
            self.segment(self.snd_nxt, tcp_flags::RST | tcp_flags::ACK, Vec::new());
        }
        self.state = State::Closed;
    }

    /// Sends what the window allows: data from `snd_nxt`, then FIN.
    fn output(&mut self, now: u64) {
        if !matches!(
            self.state,
            State::Established
                | State::CloseWait
                | State::FinWait1
                | State::LastAck
                | State::Closing
        ) {
            return;
        }
        // A zero window still lets one byte through when nothing is in
        // flight (a window probe, RFC 793 3.7).
        let window = if self.snd_wnd == 0 && self.snd_nxt == self.snd_una {
            1
        } else {
            self.snd_wnd
        };
        loop {
            let sent = self.snd_nxt.wrapping_sub(self.snd_una) as usize;
            let in_flight = sent.min(self.send_buf.len());
            let unsent = self.send_buf.len() - in_flight;
            let room = (window as usize).saturating_sub(sent);
            let n = unsent.min(room).min(usize::from(self.mss));
            if n == 0 || self.fin_seq.is_some() {
                break;
            }
            let data: Vec<u8> = self
                .send_buf
                .range(in_flight..in_flight + n)
                .copied()
                .collect();
            let seq = self.snd_nxt;
            self.segment(seq, tcp_flags::ACK | tcp_flags::PSH, data);
            self.snd_nxt = self.snd_nxt.wrapping_add(n as u32);
        }
        let all_sent = self.snd_nxt.wrapping_sub(self.snd_una) as usize >= self.send_buf.len();
        if self.fin_queued && self.fin_seq.is_none() && all_sent {
            let seq = self.snd_nxt;
            self.segment(seq, tcp_flags::FIN | tcp_flags::ACK, Vec::new());
            self.fin_seq = Some(seq);
            self.snd_nxt = seq.wrapping_add(1);
            self.state = match self.state {
                State::CloseWait => State::LastAck,
                State::Established => State::FinWait1,
                s => s,
            };
        }
        if self.snd_nxt != self.snd_una && self.retransmit_at.is_none() {
            self.retransmit_at = Some(now + self.rto_ms);
        }
    }

    /// Handles a segment from the peer.
    pub fn on_segment(&mut self, seg: &Incoming, now: u64) {
        use tcp_flags::{ACK, FIN, RST, SYN};
        if seg.flags & RST != 0 {
            match self.state {
                State::SynSent if seg.flags & ACK != 0 && seg.ack == self.iss.wrapping_add(1) => {
                    self.error = Some(TcpError::Refused);
                    self.state = State::Closed;
                }
                State::SynSent | State::Closed => {}
                _ if seg.seq == self.rcv_nxt || self.state == State::SynReceived => {
                    self.error = Some(TcpError::Reset);
                    self.state = State::Closed;
                }
                _ => {}
            }
            return;
        }
        match self.state {
            State::Closed => return,
            State::SynSent => {
                if seg.flags & (SYN | ACK) == SYN | ACK && seg.ack == self.iss.wrapping_add(1) {
                    self.snd_una = seg.ack;
                    self.rcv_nxt = seg.seq.wrapping_add(1);
                    self.snd_wnd = u32::from(seg.window);
                    self.mss = seg.mss.unwrap_or(DEFAULT_MSS).min(MSS);
                    self.state = State::Established;
                    self.retransmit_at = None;
                    self.retries = 0;
                    self.rto_ms = RTO_MIN_MS;
                    self.send_ack();
                    self.output(now);
                }
                return;
            }
            State::SynReceived => {
                if seg.flags & SYN != 0 && seg.seq.wrapping_add(1) == self.rcv_nxt {
                    // Our SYN+ACK was lost: send it again.
                    self.send_syn(now);
                    return;
                }
                if seg.flags & ACK == 0 || seg.ack != self.iss.wrapping_add(1) {
                    return;
                }
                self.snd_una = seg.ack;
                self.snd_wnd = u32::from(seg.window);
                self.state = State::Established;
                self.retransmit_at = None;
                self.retries = 0;
                self.rto_ms = RTO_MIN_MS;
            }
            _ => {}
        }
        let mut need_ack = false;
        // The acknowledgment.
        if seg.flags & ACK != 0 {
            if lt(self.snd_una, seg.ack) && le(seg.ack, self.snd_nxt) {
                let mut acked = seg.ack.wrapping_sub(self.snd_una) as usize;
                if let Some(fin) = self.fin_seq
                    && seg.ack == fin.wrapping_add(1)
                {
                    acked -= 1;
                    self.state = match self.state {
                        State::FinWait1 => State::FinWait2,
                        State::Closing => {
                            self.time_wait_until = now + TIME_WAIT_MS;
                            State::TimeWait
                        }
                        State::LastAck => State::Closed,
                        s => s,
                    };
                }
                let acked = acked.min(self.send_buf.len());
                self.send_buf.drain(..acked);
                self.snd_una = seg.ack;
                self.retries = 0;
                self.rto_ms = RTO_MIN_MS;
                self.retransmit_at = (self.snd_nxt != self.snd_una).then_some(now + self.rto_ms);
            }
            if le(self.snd_una, seg.ack) {
                self.snd_wnd = u32::from(seg.window);
            }
        }
        // The data, in order only.
        let receiving = matches!(
            self.state,
            State::Established | State::FinWait1 | State::FinWait2
        );
        if !seg.data.is_empty() {
            if receiving && seg.seq == self.rcv_nxt {
                let n = seg.data.len().min(BUFFER - self.recv_buf.len());
                self.recv_buf.extend(&seg.data[..n]);
                self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
            }
            need_ack = true;
        }
        // The end of their data.
        if seg.flags & FIN != 0 {
            let fin_at = seg.seq.wrapping_add(seg.data.len() as u32);
            if fin_at == self.rcv_nxt && !self.peer_fin && self.synchronised() {
                self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                self.peer_fin = true;
                self.state = match self.state {
                    State::Established => State::CloseWait,
                    State::FinWait1 => State::Closing,
                    State::FinWait2 => {
                        self.time_wait_until = now + TIME_WAIT_MS;
                        State::TimeWait
                    }
                    s => s,
                };
            }
            need_ack = true;
        }
        if self.state == State::TimeWait && (need_ack || seg.flags & FIN != 0) {
            self.time_wait_until = now + TIME_WAIT_MS;
        }
        if need_ack && self.state != State::Closed {
            self.send_ack();
        }
        self.output(now);
    }

    /// Timers: retransmission and the end of TIME-WAIT.
    pub fn poll(&mut self, now: u64) {
        if self.state == State::TimeWait && now >= self.time_wait_until {
            self.state = State::Closed;
            return;
        }
        let Some(at) = self.retransmit_at else {
            return;
        };
        if now < at {
            return;
        }
        self.retries += 1;
        if self.retries > MAX_RETRIES {
            self.error = Some(TcpError::TimedOut);
            self.state = State::Closed;
            self.retransmit_at = None;
            return;
        }
        self.rto_ms = (self.rto_ms * 2).min(RTO_MAX_MS);
        self.retransmit_at = None;
        match self.state {
            State::SynSent | State::SynReceived => self.send_syn(now),
            _ => {
                // Go back to the oldest unacknowledged byte.
                self.snd_nxt = self.snd_una;
                if self
                    .fin_seq
                    .is_some_and(|f| lt(self.snd_una, f.wrapping_add(1)))
                {
                    self.fin_seq = None;
                }
                self.output(now);
            }
        }
    }

    /// Whether the connection is gone for good.
    pub fn is_closed(&self) -> bool {
        self.state == State::Closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tcp_flags::{ACK, FIN, PSH, RST, SYN};

    const PEER: Ip = [10, 0, 2, 2];

    fn incoming(seg: &Segment) -> Incoming<'_> {
        Incoming {
            seq: seg.seq,
            ack: seg.ack,
            flags: seg.flags,
            window: seg.window,
            mss: seg.mss,
            data: &seg.data,
        }
    }

    /// Delivers every segment queued in `from` to `to`.
    fn deliver(from: &mut Tcb, to: &mut Tcb, now: u64) -> usize {
        let out: Vec<Segment> = core::mem::take(&mut from.out);
        for s in &out {
            to.on_segment(&incoming(s), now);
        }
        out.len()
    }

    /// A connected pair: (client, server).
    fn pair() -> (Tcb, Tcb) {
        let mut c = Tcb::connect(49152, PEER, 80, 1000, 0);
        assert_eq!(c.state, State::SynSent);
        let syn = c.out.remove(0);
        assert_eq!(syn.flags, SYN);
        assert_eq!(syn.mss, Some(MSS));
        let mut s = Tcb::accept(80, [10, 0, 2, 15], 49152, &incoming(&syn), 5000, 0);
        assert_eq!(s.state, State::SynReceived);
        assert_eq!(s.out[0].flags, SYN | ACK);
        assert_eq!(s.out[0].ack, 1001);
        deliver(&mut s, &mut c, 1);
        assert_eq!(c.state, State::Established);
        deliver(&mut c, &mut s, 2);
        assert_eq!(s.state, State::Established);
        (c, s)
    }

    #[test]
    fn handshake_and_data_both_ways() {
        let (mut c, mut s) = pair();
        assert_eq!(c.send(b"GET / HTTP/1.0\r\n\r\n", 3), 18);
        deliver(&mut c, &mut s, 4);
        let mut buf = [0u8; 64];
        assert_eq!(s.recv(&mut buf), 18);
        assert_eq!(&buf[..18], b"GET / HTTP/1.0\r\n\r\n");
        // The server's ACK frees the client's buffer.
        deliver(&mut s, &mut c, 5);
        assert_eq!(c.send_room(), BUFFER);
        assert_eq!(c.retransmit_at, None);
        s.send(b"hello", 6);
        deliver(&mut s, &mut c, 7);
        assert_eq!(c.recv(&mut buf), 5);
        assert_eq!(&buf[..5], b"hello");
    }

    #[test]
    fn large_transfer_is_segmented() {
        let (mut c, mut s) = pair();
        let data: Vec<u8> = (0..20_000u32).map(|i| i as u8).collect();
        assert_eq!(s.send(&data, 3), 20_000);
        let segs = s.out.iter().filter(|x| !x.data.is_empty()).count();
        assert_eq!(segs, 20_000usize.div_ceil(usize::from(MSS)));
        assert!(s.out.iter().all(|x| x.data.len() <= usize::from(MSS)));
        deliver(&mut s, &mut c, 4);
        let mut got = alloc::vec![0u8; 30_000];
        assert_eq!(c.recv(&mut got), 20_000);
        assert_eq!(&got[..20_000], &data[..]);
    }

    #[test]
    fn window_limits_sending() {
        let (mut c, mut s) = pair();
        // The client advertises 4096 bytes.
        let mut small = c.out.clone();
        small.push(Segment {
            seq: 1001,
            ack: 5001,
            flags: ACK,
            window: 4096,
            mss: None,
            data: Vec::new(),
        });
        for seg in &small {
            s.on_segment(&incoming(seg), 3);
        }
        s.out.clear();
        s.send(&[7u8; 10_000], 4);
        let sent: usize = s.out.iter().map(|x| x.data.len()).sum();
        assert_eq!(sent, 4096);
        let _ = c.out.drain(..);
        let _ = &mut c;
    }

    #[test]
    fn lost_segment_is_retransmitted() {
        let (mut c, mut s) = pair();
        c.send(b"abc", 10);
        let lost = c.out.remove(0);
        assert_eq!(lost.flags, ACK | PSH);
        // Nothing before the timeout.
        c.poll(500);
        assert!(c.out.is_empty());
        c.poll(10 + RTO_MIN_MS);
        assert_eq!(c.out.len(), 1);
        assert_eq!(c.out[0].data, b"abc");
        assert_eq!(c.out[0].seq, lost.seq);
        deliver(&mut c, &mut s, 1100);
        let mut buf = [0u8; 8];
        assert_eq!(s.recv(&mut buf), 3);
    }

    #[test]
    fn out_of_order_is_dropped_and_duplicate_acked() {
        let (mut c, mut s) = pair();
        c.send(b"first", 3);
        c.send(b"second", 3);
        let first = c.out.remove(0);
        let second = c.out.remove(0);
        s.on_segment(&incoming(&second), 4);
        assert_eq!(s.readable(), 0);
        assert_eq!(s.out.last().map(|x| x.ack), Some(1001));
        s.on_segment(&incoming(&first), 5);
        assert_eq!(s.readable(), 5);
    }

    #[test]
    fn retransmission_gives_up() {
        let mut c = Tcb::connect(49152, PEER, 80, 1, 0);
        let mut now = 0;
        for _ in 0..20 {
            now += RTO_MAX_MS;
            c.poll(now);
        }
        assert_eq!(c.state, State::Closed);
        assert_eq!(c.error, Some(TcpError::TimedOut));
    }

    #[test]
    fn refused_connection() {
        let mut c = Tcb::connect(49152, PEER, 81, 1, 0);
        c.on_segment(
            &Incoming {
                seq: 0,
                ack: 2,
                flags: RST | ACK,
                window: 0,
                mss: None,
                data: &[],
            },
            1,
        );
        assert_eq!(c.error, Some(TcpError::Refused));
        assert!(c.is_closed());
    }

    #[test]
    fn active_close() {
        let (mut c, mut s) = pair();
        c.close(3);
        assert_eq!(c.state, State::FinWait1);
        assert_eq!(c.out.last().map(|x| x.flags), Some(FIN | ACK));
        deliver(&mut c, &mut s, 4);
        assert_eq!(s.state, State::CloseWait);
        assert!(s.at_end());
        deliver(&mut s, &mut c, 5);
        assert_eq!(c.state, State::FinWait2);
        s.close(6);
        assert_eq!(s.state, State::LastAck);
        deliver(&mut s, &mut c, 7);
        assert_eq!(c.state, State::TimeWait);
        deliver(&mut c, &mut s, 8);
        assert_eq!(s.state, State::Closed);
        c.poll(8 + TIME_WAIT_MS);
        assert_eq!(c.state, State::Closed);
    }

    #[test]
    fn close_after_data_sends_fin_last() {
        let (mut c, mut s) = pair();
        s.send(b"body", 3);
        s.close(3);
        let flags: Vec<u8> = s.out.iter().map(|x| x.flags).collect();
        assert_eq!(flags.last(), Some(&(FIN | ACK)));
        deliver(&mut s, &mut c, 4);
        let mut buf = [0u8; 8];
        assert_eq!(c.recv(&mut buf), 4);
        assert!(c.at_end());
    }

    #[test]
    fn lost_fin_is_retransmitted() {
        let (mut c, mut s) = pair();
        c.close(10);
        c.out.clear();
        c.poll(10 + RTO_MIN_MS);
        assert_eq!(c.out.last().map(|x| x.flags), Some(FIN | ACK));
        deliver(&mut c, &mut s, 1100);
        assert_eq!(s.state, State::CloseWait);
    }

    #[test]
    fn reset_ends_an_open_connection() {
        let (mut c, mut s) = pair();
        s.abort();
        assert_eq!(s.out.last().map(|x| x.flags & RST), Some(RST));
        deliver(&mut s, &mut c, 3);
        assert_eq!(c.error, Some(TcpError::Reset));
        assert!(c.at_end());
    }

    #[test]
    fn sequence_numbers_wrap() {
        let mut c = Tcb::connect(1, PEER, 2, u32::MAX - 1, 0);
        let syn = c.out.remove(0);
        let mut s = Tcb::accept(2, PEER, 1, &incoming(&syn), u32::MAX, 0);
        deliver(&mut s, &mut c, 1);
        deliver(&mut c, &mut s, 1);
        assert_eq!((c.state, s.state), (State::Established, State::Established));
        c.send(&[1; 100], 2);
        deliver(&mut c, &mut s, 3);
        deliver(&mut s, &mut c, 3);
        assert_eq!(s.readable(), 100);
        assert_eq!(c.send_room(), BUFFER);
    }
}
