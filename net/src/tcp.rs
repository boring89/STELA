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
//! P11 (`docs/net.md` §5.1):
//!
//! - **RTT** (RFC 6298): one segment timed at a time (not a retransmitted
//!   one, Karn's rule); SRTT and RTTVAR give RTO = SRTT + 4 RTTVAR, at
//!   least 1 s, doubling on each timeout up to 8 s, 8 tries at most.
//! - **Congestion control** (RFC 5681): slow start from 3 segments,
//!   congestion avoidance past ssthresh; a timeout drops to one segment
//!   and goes back to the oldest unacknowledged byte.
//! - **Fast retransmit and recovery**: three duplicate ACKs resend the
//!   oldest unacknowledged segment at once and halve the window.
//! - **Delayed ACK** (RFC 1122 4.2.3.2): every second full segment, or
//!   200 ms after the first; out-of-order data, FIN and a window that
//!   opens again are acknowledged at once.
//! - **Reassembly**: segments past a gap are kept (inside the receive
//!   window, the 64 KiB buffer) until the gap is filled.

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
/// The delayed ACK's timer (RFC 1122 asks for less than 500 ms).
pub const ACK_DELAY_MS: u64 = 200;
/// The initial congestion window, in segments (RFC 5681 3.1: 3 for an
/// MSS of 1460).
const INITIAL_SEGMENTS: u32 = 3;
/// Duplicate ACKs that start a fast retransmit.
const DUP_ACKS: u32 = 3;
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
    /// Smoothed round-trip time and its variation (ms), once measured.
    srtt_ms: Option<u64>,
    rttvar_ms: u64,
    /// The segment being timed: (sequence number its ACK must pass, sent
    /// at).
    rtt_probe: Option<(u32, u64)>,
    /// Congestion window and slow-start threshold (bytes).
    pub cwnd: u32,
    ssthresh: u32,
    /// Duplicate ACKs in a row, and whether fast recovery runs (until
    /// `recover` is acknowledged).
    dup_acks: u32,
    recovering: bool,
    recover: u32,
    /// In-order segments received since our last ACK, and when the
    /// delayed ACK is due.
    unacked: u32,
    ack_at: Option<u64>,
    /// Segments past a gap: (sequence number, data), in order.
    ooo: Vec<(u32, Vec<u8>)>,
    /// Their FIN's sequence number when it came past a gap.
    fin_at: Option<u32>,
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
            srtt_ms: None,
            rttvar_ms: 0,
            rtt_probe: None,
            cwnd: INITIAL_SEGMENTS * u32::from(DEFAULT_MSS),
            ssthresh: 0xFFFF,
            dup_acks: 0,
            recovering: false,
            recover: iss,
            unacked: 0,
            ack_at: None,
            ooo: Vec::new(),
            fin_at: None,
        }
    }

    /// The congestion window once the MSS is known.
    fn start_window(&mut self) {
        self.cwnd = INITIAL_SEGMENTS * u32::from(self.mss);
    }

    /// The current retransmission timeout (ms).
    pub fn rto(&self) -> u64 {
        self.rto_ms
    }

    /// The smoothed round-trip time (ms), once measured.
    pub fn srtt(&self) -> Option<u64> {
        self.srtt_ms
    }

    /// A round-trip sample of `r` ms (RFC 6298 2.2, 2.3; G = 1 ms).
    fn rtt_sample(&mut self, r: u64) {
        match self.srtt_ms {
            None => {
                self.srtt_ms = Some(r);
                self.rttvar_ms = r / 2;
            }
            Some(srtt) => {
                let diff = srtt.abs_diff(r);
                self.rttvar_ms = (3 * self.rttvar_ms + diff) / 4;
                self.srtt_ms = Some((7 * srtt + r) / 8);
            }
        }
        let srtt = self.srtt_ms.unwrap_or(r);
        self.rto_ms = (srtt + (4 * self.rttvar_ms).max(1)).clamp(RTO_MIN_MS, RTO_MAX_MS);
    }

    /// Bytes sent and not yet acknowledged.
    fn flight(&self) -> u32 {
        self.snd_nxt.wrapping_sub(self.snd_una)
    }

    /// Halves the window after a loss (RFC 5681 (4)).
    fn loss(&mut self) {
        self.ssthresh = (self.flight() / 2).max(2 * u32::from(self.mss));
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
        t.start_window();
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
        if flags & tcp_flags::ACK != 0 {
            // Every segment acknowledges what came: no ACK is pending.
            self.unacked = 0;
            self.ack_at = None;
        }
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
        // Time the SYN unless it is a retransmission.
        self.rtt_probe = (self.retries == 0).then_some((self.snd_nxt, now));
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
        // flight (a window probe, RFC 793 3.7). The congestion window
        // limits it too.
        let window = if self.snd_wnd == 0 && self.snd_nxt == self.snd_una {
            1
        } else {
            self.snd_wnd.min(self.cwnd.max(u32::from(self.mss)))
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
            // Time this segment (only new data: Karn's rule).
            if self.rtt_probe.is_none() && self.retries == 0 && !self.recovering {
                self.rtt_probe = Some((self.snd_nxt, now));
            }
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

    /// The RTO from the estimate (or the initial 1 s): after progress,
    /// the backed-off timer is forgotten (RFC 6298 5.7).
    fn reset_rto(&mut self) {
        self.rto_ms = match self.srtt_ms {
            Some(srtt) => (srtt + (4 * self.rttvar_ms).max(1)).clamp(RTO_MIN_MS, RTO_MAX_MS),
            None => RTO_MIN_MS,
        };
    }

    /// An ACK that takes `ack` past `snd_una` (`ack` is valid).
    fn new_ack(&mut self, ack: u32, now: u64) {
        let mut acked = ack.wrapping_sub(self.snd_una) as usize;
        if let Some(fin) = self.fin_seq
            && ack == fin.wrapping_add(1)
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
        let newly = ack.wrapping_sub(self.snd_una);
        let acked = acked.min(self.send_buf.len());
        self.send_buf.drain(..acked);
        self.snd_una = ack;
        // The round trip of the segment being timed.
        if let Some((end, sent)) = self.rtt_probe
            && le(end, ack)
        {
            self.rtt_probe = None;
            self.rtt_sample(now.saturating_sub(sent));
        } else {
            self.reset_rto();
        }
        self.retries = 0;
        self.dup_acks = 0;
        let mss = u32::from(self.mss);
        if self.recovering {
            if le(self.recover, ack) {
                // Fast recovery done (RFC 5681 3.2 step 6).
                self.recovering = false;
                self.cwnd = self.ssthresh;
            } else {
                // A partial ACK: the next hole goes out at once (RFC 6582).
                self.cwnd = self.cwnd.saturating_sub(newly).max(mss) + mss;
                self.resend_oldest();
            }
        } else if self.cwnd < self.ssthresh {
            // Slow start (RFC 5681 3.1, (2)).
            self.cwnd += newly.min(mss);
        } else {
            // Congestion avoidance (RFC 5681 (3)).
            self.cwnd += (mss * mss / self.cwnd.max(1)).max(1);
        }
        self.retransmit_at = (self.snd_nxt != self.snd_una).then_some(now + self.rto_ms);
    }

    /// Sends the oldest unacknowledged segment again (fast retransmit).
    fn resend_oldest(&mut self) {
        let n = self.send_buf.len().min(usize::from(self.mss));
        if n == 0 {
            return;
        }
        let data: Vec<u8> = self.send_buf.range(..n).copied().collect();
        let seq = self.snd_una;
        self.segment(seq, tcp_flags::ACK | tcp_flags::PSH, data);
        // That segment is not timed any more (Karn's rule).
        self.rtt_probe = None;
    }

    /// In-order data: into the receive buffer, then whatever kept
    /// segments it reaches.
    fn take_data(&mut self, seq: u32, data: &[u8]) -> bool {
        // Drop what we already have.
        let skip = self.rcv_nxt.wrapping_sub(seq) as usize;
        if lt(self.rcv_nxt, seq) || skip >= data.len() {
            return false;
        }
        let data = &data[skip..];
        let n = data.len().min(BUFFER - self.recv_buf.len());
        self.recv_buf.extend(&data[..n]);
        self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
        // The kept segments the gap's closing reaches.
        while let Some(i) = self.ooo.iter().position(|(s, _)| le(*s, self.rcv_nxt)) {
            let (s, d) = self.ooo.remove(i);
            let skip = self.rcv_nxt.wrapping_sub(s) as usize;
            if skip < d.len() {
                let rest = &d[skip..];
                let n = rest.len().min(BUFFER - self.recv_buf.len());
                self.recv_buf.extend(&rest[..n]);
                self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
            }
        }
        true
    }

    /// A segment past a gap: kept (inside the window) for later.
    fn keep(&mut self, seq: u32, data: &[u8]) {
        let end = seq.wrapping_add(data.len() as u32);
        let limit = self.rcv_nxt.wrapping_add(u32::from(self.window()));
        let stored: usize = self.ooo.iter().map(|(_, d)| d.len()).sum();
        if !le(end, limit)
            || stored + data.len() + self.recv_buf.len() > BUFFER
            || self.ooo.iter().any(|(s, _)| *s == seq)
        {
            return;
        }
        let at = self
            .ooo
            .iter()
            .position(|(s, _)| lt(seq, *s))
            .unwrap_or(self.ooo.len());
        self.ooo.insert(at, (seq, data.to_vec()));
    }

    /// Their FIN, once all the data before it is here.
    fn take_fin(&mut self, now: u64) {
        let Some(at) = self.fin_at else {
            return;
        };
        if at != self.rcv_nxt || self.peer_fin || !self.synchronised() {
            return;
        }
        self.fin_at = None;
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
                    self.start_window();
                    self.state = State::Established;
                    self.handshake_done(now);
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
                self.handshake_done(now);
            }
            _ => {}
        }
        let mut ack_now = false;
        // The acknowledgment.
        if seg.flags & ACK != 0 {
            if lt(self.snd_una, seg.ack) && le(seg.ack, self.snd_nxt) {
                self.new_ack(seg.ack, now);
            } else if seg.ack == self.snd_una
                && seg.data.is_empty()
                && seg.flags & FIN == 0
                && self.snd_nxt != self.snd_una
                && u32::from(seg.window) == self.snd_wnd
            {
                // A duplicate ACK (RFC 5681 2): the peer got something
                // past a hole.
                self.dup_acks += 1;
                let mss = u32::from(self.mss);
                if self.dup_acks == DUP_ACKS && !self.recovering {
                    self.loss();
                    self.recovering = true;
                    self.recover = self.snd_nxt;
                    self.resend_oldest();
                    self.cwnd = self.ssthresh + DUP_ACKS * mss;
                } else if self.recovering {
                    // Each one means a segment left the network.
                    self.cwnd += mss;
                }
            }
            if le(self.snd_una, seg.ack) {
                self.snd_wnd = u32::from(seg.window);
            }
        }
        // The data: in order goes in, past a gap is kept.
        let receiving = matches!(
            self.state,
            State::Established | State::FinWait1 | State::FinWait2
        );
        if !seg.data.is_empty() {
            if receiving && le(seg.seq, self.rcv_nxt) {
                let had_gap = !self.ooo.is_empty();
                if self.take_data(seg.seq, seg.data) {
                    self.unacked += 1;
                    // Every second segment, and at once when it filled a
                    // gap (RFC 5681 4.2).
                    if self.unacked >= 2 || had_gap {
                        ack_now = true;
                    } else if self.ack_at.is_none() {
                        self.ack_at = Some(now + ACK_DELAY_MS);
                    }
                } else {
                    // Old data again: tell the peer where we are.
                    ack_now = true;
                }
            } else {
                if receiving {
                    self.keep(seg.seq, seg.data);
                }
                // Out of order: a duplicate ACK at once.
                ack_now = true;
            }
        }
        // The end of their data.
        if seg.flags & FIN != 0 && self.synchronised() {
            if !self.peer_fin {
                self.fin_at = Some(seg.seq.wrapping_add(seg.data.len() as u32));
            }
            ack_now = true;
        }
        self.take_fin(now);
        if self.state == State::TimeWait && (ack_now || seg.flags & FIN != 0) {
            self.time_wait_until = now + TIME_WAIT_MS;
        }
        if ack_now && self.state != State::Closed {
            self.send_ack();
        }
        self.output(now);
    }

    /// The handshake finished: its round trip is the first sample.
    fn handshake_done(&mut self, now: u64) {
        if let Some((_, sent)) = self.rtt_probe.take()
            && self.retries == 0
        {
            self.rtt_sample(now.saturating_sub(sent));
        } else {
            self.reset_rto();
        }
        self.retransmit_at = None;
        self.retries = 0;
    }

    /// Timers: retransmission, the delayed ACK and the end of TIME-WAIT.
    pub fn poll(&mut self, now: u64) {
        if self.state == State::TimeWait && now >= self.time_wait_until {
            self.state = State::Closed;
            return;
        }
        if self.ack_at.is_some_and(|at| now >= at) && self.synchronised() {
            self.send_ack();
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
        self.rtt_probe = None;
        match self.state {
            State::SynSent | State::SynReceived => self.send_syn(now),
            _ => {
                // A timeout: one segment's window, back to the oldest
                // unacknowledged byte (RFC 5681 3.1, (4), (5)).
                self.loss();
                self.cwnd = u32::from(self.mss);
                self.recovering = false;
                self.dup_acks = 0;
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

    /// When the next timer is due (retransmission or the delayed ACK).
    pub fn next_timer(&self) -> Option<u64> {
        match (self.retransmit_at, self.ack_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
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
        // One segment: the ACK is delayed (RFC 1122 4.2.3.2).
        assert!(s.out.is_empty());
        s.poll(4 + ACK_DELAY_MS - 1);
        assert!(s.out.is_empty());
        s.poll(4 + ACK_DELAY_MS);
        // The server's ACK frees the client's buffer.
        deliver(&mut s, &mut c, 5);
        assert_eq!(c.send_room(), BUFFER);
        assert_eq!(c.retransmit_at, None);
        s.send(b"hello", 6);
        deliver(&mut s, &mut c, 7);
        assert_eq!(c.recv(&mut buf), 5);
        assert_eq!(&buf[..5], b"hello");
    }

    /// Delivers both ways (with the delayed ACK's timer) until quiet.
    fn settle(c: &mut Tcb, s: &mut Tcb, now: &mut u64) {
        for _ in 0..1000 {
            *now += 1;
            let moved = deliver(c, s, *now) + deliver(s, c, *now);
            if moved == 0 {
                *now += ACK_DELAY_MS;
                c.poll(*now);
                s.poll(*now);
                if c.out.is_empty() && s.out.is_empty() {
                    return;
                }
            }
        }
        panic!("the connection never went quiet");
    }

    #[test]
    fn large_transfer_is_segmented() {
        let (mut c, mut s) = pair();
        let data: Vec<u8> = (0..20_000u32).map(|i| i as u8).collect();
        assert_eq!(s.send(&data, 3), 20_000);
        // Slow start: three segments first (RFC 5681 3.1).
        let segs = s.out.iter().filter(|x| !x.data.is_empty()).count();
        assert_eq!(segs, 3);
        assert!(s.out.iter().all(|x| x.data.len() <= usize::from(MSS)));
        let mut now = 3;
        settle(&mut c, &mut s, &mut now);
        let mut got = alloc::vec![0u8; 30_000];
        assert_eq!(c.recv(&mut got), 20_000);
        assert_eq!(&got[..20_000], &data[..]);
        // The window grew past the initial one.
        assert!(s.cwnd > 3 * u32::from(MSS));
    }

    #[test]
    fn every_second_segment_is_acknowledged() {
        let (mut c, mut s) = pair();
        s.send(&[5u8; 2 * MSS as usize], 3);
        assert_eq!(s.out.len(), 2);
        let first = s.out.remove(0);
        c.on_segment(&incoming(&first), 4);
        assert!(c.out.is_empty());
        let second = s.out.remove(0);
        c.on_segment(&incoming(&second), 5);
        assert_eq!(c.out.len(), 1);
        assert_eq!(c.out[0].ack, second.seq + u32::from(MSS));
    }

    #[test]
    fn three_duplicate_acks_resend_at_once() {
        let (mut c, mut s) = pair();
        // Open the window first.
        let mut now = 3;
        s.send(&[1u8; 20_000], now);
        settle(&mut c, &mut s, &mut now);
        let mut sink = alloc::vec![0u8; 30_000];
        c.recv(&mut sink);
        s.send(&[2u8; 6 * MSS as usize], now);
        let mut segs: Vec<Segment> = core::mem::take(&mut s.out);
        assert!(segs.len() >= 5, "{}", segs.len());
        let lost = segs.remove(0);
        for seg in &segs {
            c.on_segment(&incoming(seg), now);
        }
        // Each out-of-order segment got a duplicate ACK at once.
        let dups: Vec<Segment> = core::mem::take(&mut c.out);
        assert!(dups.iter().all(|d| d.ack == lost.seq));
        for d in &dups {
            s.on_segment(&incoming(d), now + 1);
        }
        // Well before the RTO, the lost segment went out again.
        assert_eq!(s.out.first().map(|x| x.seq), Some(lost.seq));
        let again = s.out.remove(0);
        c.on_segment(&incoming(&again), now + 2);
        // The gap is filled: everything kept is readable.
        assert_eq!(c.readable(), 6 * usize::from(MSS));
        assert_eq!(
            c.out.last().map(|x| x.ack),
            Some(lost.seq + 6 * u32::from(MSS))
        );
    }

    #[test]
    fn round_trip_time_sets_the_timeout() {
        let mut c = Tcb::connect(49152, PEER, 80, 1000, 0);
        let syn = c.out.remove(0);
        let mut s = Tcb::accept(80, [10, 0, 2, 15], 49152, &incoming(&syn), 5000, 0);
        // A 1.5 s round trip for the handshake.
        deliver(&mut s, &mut c, 1500);
        assert_eq!(c.srtt(), Some(1500));
        assert_eq!(c.rto(), RTO_MAX_MS.min(1500 + 4 * 750));
        deliver(&mut c, &mut s, 1500);
        // Then 300 ms for data: the estimate moves toward it.
        c.send(b"x", 2000);
        deliver(&mut c, &mut s, 2000);
        s.poll(2000 + ACK_DELAY_MS);
        deliver(&mut s, &mut c, 2300);
        let srtt = c.srtt().unwrap();
        assert!(srtt < 1500 && srtt > 300, "{srtt}");
        assert!(c.rto() >= RTO_MIN_MS);
    }

    #[test]
    fn fin_after_a_gap_waits_for_the_data() {
        let (mut c, mut s) = pair();
        s.send(b"abc", 3);
        s.send(b"def", 3);
        s.close(3);
        let segs: Vec<Segment> = core::mem::take(&mut s.out);
        // "def" and the FIN first, "abc" last.
        c.on_segment(&incoming(&segs[1]), 4);
        c.on_segment(&incoming(&segs[2]), 4);
        assert!(!c.peer_fin);
        assert_eq!(c.readable(), 0);
        c.on_segment(&incoming(&segs[0]), 5);
        assert_eq!(c.readable(), 6);
        assert!(c.peer_fin);
        assert_eq!(c.state, State::CloseWait);
        let mut buf = [0u8; 8];
        assert_eq!(c.recv(&mut buf), 6);
        assert_eq!(&buf[..6], b"abcdef");
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
    fn out_of_order_is_kept_and_duplicate_acked() {
        let (mut c, mut s) = pair();
        c.send(b"first", 3);
        c.send(b"second", 3);
        let first = c.out.remove(0);
        let second = c.out.remove(0);
        s.on_segment(&incoming(&second), 4);
        assert_eq!(s.readable(), 0);
        assert_eq!(s.out.last().map(|x| x.ack), Some(1001));
        // The gap is filled: both are there, acknowledged at once.
        s.on_segment(&incoming(&first), 5);
        assert_eq!(s.readable(), 11);
        assert_eq!(s.out.last().map(|x| x.ack), Some(1012));
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
        // The ACK of one segment waits for the delayed ACK's timer.
        s.poll(3 + ACK_DELAY_MS);
        deliver(&mut s, &mut c, 3 + ACK_DELAY_MS);
        assert_eq!(s.readable(), 100);
        assert_eq!(c.send_room(), BUFFER);
    }
}
