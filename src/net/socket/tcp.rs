use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Event;
use crate::kernel::scheduler::current;
use crate::klib::SpinLock;
use crate::net::interface::Interface;
use crate::net::manager;
use crate::net::protocol::{TCPPacket, TcpFlags};

use super::inet::InetSocket;
use super::{SHUT_RDWR, SHUT_WR, SocketAddr, SocketInner};

const DEFAULT_WINDOW: u16 = 65535;
const MSS: usize = 1460;
const DUP_ACK_THRESHOLD: u32 = 3;
const MAX_RETRANSMITS: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// Pending connection on a listening socket (completed 3-way handshake).
struct PendingConn {
    local: SocketAddr,
    remote: SocketAddr,
    snd_nxt: u32,
    rcv_nxt: u32,
    iface: Arc<Interface>,
}

pub struct TcpInner {
    state: TcpState,
    local: Option<SocketAddr>,
    remote: Option<SocketAddr>,
    iface: Option<Arc<Interface>>,

    // === Send sequence space ===
    //   snd_una: oldest unacknowledged byte
    //   snd_nxt: next byte to send
    //   tx_buf:  bytes [snd_una, snd_nxt) kept for retransmission
    //   snd_wnd: remote advertised window size
    snd_una: u32,
    snd_nxt: u32,
    tx_buf: VecDeque<u8>,
    snd_wnd: u32,

    // === Receive sequence space ===
    //   rcv_nxt:    next expected byte from remote
    //   rx_buf:     in-order reassembled data ready for application
    //   ooo_segs:   out-of-order segments indexed by seq number
    //   rx_closed:  remote sent FIN
    rcv_nxt: u32,
    rx_buf: VecDeque<u8>,
    ooo_segs: BTreeMap<u32, Vec<u8>>,
    rx_closed: bool,

    // === Retransmission ===
    dup_ack_count: u32,
    retransmit_count: u32,

    // Transmit state
    tx_closed: bool,

    // Listen backlog
    backlog: Option<Arc<SpinLock<AcceptQueue>>>,
}

struct AcceptQueue {
    queue: VecDeque<PendingConn>,
    capacity: usize,
}

impl TcpInner {
    pub fn new() -> Self {
        let isn = initial_seq();
        Self {
            state: TcpState::Closed,
            local: None,
            remote: None,
            iface: None,
            snd_una: isn,
            snd_nxt: isn,
            tx_buf: VecDeque::new(),
            snd_wnd: DEFAULT_WINDOW as u32,
            rcv_nxt: 0,
            rx_buf: VecDeque::new(),
            ooo_segs: BTreeMap::new(),
            rx_closed: false,
            dup_ack_count: 0,
            retransmit_count: 0,
            tx_closed: false,
            backlog: None,
        }
    }

    /// Create an established socket from an accepted connection.
    fn from_accepted(conn: PendingConn) -> Self {
        let isn = conn.snd_nxt;
        Self {
            state: TcpState::Established,
            local: Some(conn.local),
            remote: Some(conn.remote),
            iface: Some(conn.iface),
            snd_una: isn,
            snd_nxt: isn,
            tx_buf: VecDeque::new(),
            snd_wnd: DEFAULT_WINDOW as u32,
            rcv_nxt: conn.rcv_nxt,
            rx_buf: VecDeque::new(),
            ooo_segs: BTreeMap::new(),
            rx_closed: false,
            dup_ack_count: 0,
            retransmit_count: 0,
            tx_closed: false,
            backlog: None,
        }
    }

    fn resolve_iface(&mut self) -> SysResult<Arc<Interface>> {
        if let Some(ref iface) = self.iface {
            return Ok(iface.clone());
        }
        let iface = manager::default_interface().ok_or(Errno::ENETUNREACH)?;
        self.iface = Some(iface.clone());
        Ok(iface)
    }

    /// Advertised receive window: how much space left in rx_buf.
    fn rcv_wnd(&self) -> u16 {
        let used = self.rx_buf.len();
        let free = (DEFAULT_WINDOW as usize).saturating_sub(used);
        free.min(u16::MAX as usize) as u16
    }

    /// Send a TCP segment with explicit seq number.
    fn send_segment_at(&self, seq: u32, flags: TcpFlags, payload: &[u8]) -> SysResult<()> {
        let iface = self.iface.as_ref().ok_or(Errno::ENOTCONN)?;
        let local = self.local.ok_or(Errno::ENOTCONN)?;
        let remote = self.remote.ok_or(Errno::ENOTCONN)?;
        iface.send_tcp(local, remote, seq, self.rcv_nxt, flags, self.rcv_wnd(), payload)
    }

    /// Send a TCP segment using snd_nxt as seq.
    fn send_segment(&self, flags: TcpFlags, payload: &[u8]) -> SysResult<()> {
        self.send_segment_at(self.snd_nxt, flags, payload)
    }

    fn send_rst(&self) {
        let _ = self.send_segment(TcpFlags::RST | TcpFlags::ACK, &[]);
    }

    /// Retransmit unacknowledged data from snd_una.
    fn retransmit(&mut self) -> SysResult<()> {
        if self.tx_buf.is_empty() {
            return Ok(());
        }
        if self.retransmit_count >= MAX_RETRANSMITS {
            self.state = TcpState::Closed;
            self.rx_closed = true;
            return Err(Errno::ETIMEDOUT);
        }

        // Retransmit the first MSS of unacked data
        let len = self.tx_buf.len().min(MSS);
        let data: Vec<u8> = self.tx_buf.iter().take(len).copied().collect();
        let flags = TcpFlags::ACK
            | if len == self.tx_buf.len() {
                TcpFlags::PSH
            } else {
                TcpFlags::empty()
            };
        self.send_segment_at(self.snd_una, flags, &data)?;
        self.retransmit_count += 1;
        Ok(())
    }

    /// How many bytes we're allowed to send (limited by remote window and unacked data).
    fn send_window_available(&self) -> usize {
        let in_flight = self.snd_nxt.wrapping_sub(self.snd_una) as usize;
        self.snd_wnd.saturating_sub(in_flight as u32) as usize
    }

    /// Wait for a TCP segment on our local port, blocking or non-blocking.
    fn recv_segment(&self, blocked: bool) -> SysResult<(SocketAddr, Vec<u8>)> {
        let local = self.local.ok_or(Errno::EINVAL)?;
        let iface = self.iface.as_ref().ok_or(Errno::EINVAL)?;

        loop {
            if let Some(entry) = iface.try_recv_tcp(local.port) {
                return Ok(entry);
            }
            if !blocked {
                return Err(Errno::EAGAIN);
            }
            iface.wait_tcp(local.port);
            current::schedule();
            match current::task().take_wakeup_event() {
                Some(Event::Signal) => {
                    iface.cancel_wait_tcp(local.port);
                    return Err(Errno::EINTR);
                }
                _ => continue,
            }
        }
    }

    /// Merge any contiguous out-of-order segments into rx_buf.
    fn merge_ooo(&mut self) {
        while let Some(entry) = self.ooo_segs.first_entry() {
            let seg_seq = *entry.key();
            let seg_end = seg_seq.wrapping_add(entry.get().len() as u32);

            if seq_before_eq(seg_seq, self.rcv_nxt) {
                // This segment overlaps or is contiguous with rcv_nxt
                let data = entry.remove();
                if seq_after(seg_end, self.rcv_nxt) {
                    // Some new bytes
                    let skip = self.rcv_nxt.wrapping_sub(seg_seq) as usize;
                    self.rx_buf.extend(&data[skip..]);
                    self.rcv_nxt = seg_end;
                }
                // else: entirely duplicate, just discard
            } else {
                break; // gap remains
            }
        }
    }

    /// Process one incoming segment and update state. Returns true if something changed.
    fn process_segment(&mut self, src: SocketAddr, data: &[u8]) -> bool {
        let pkt = match TCPPacket::parse(data) {
            Some(p) => p,
            None => return false,
        };

        let flags = pkt.flags();
        let seq = pkt.seq_num();
        let ack = pkt.ack_num();
        let payload = pkt.payload();
        let wnd = pkt.window_size() as u32;

        match self.state {
            TcpState::SynSent => {
                if flags.contains(TcpFlags::SYN | TcpFlags::ACK) {
                    // Validate: ACK must acknowledge our SYN
                    if ack != self.snd_nxt {
                        return false;
                    }
                    self.snd_una = ack;
                    self.rcv_nxt = seq.wrapping_add(1);
                    self.snd_wnd = wnd;
                    self.remote = Some(src);
                    self.state = TcpState::Established;
                    let _ = self.send_segment(TcpFlags::ACK, &[]);
                    return true;
                }
                if flags.contains(TcpFlags::RST) {
                    self.state = TcpState::Closed;
                    return true;
                }
            }

            TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2 => {
                if flags.contains(TcpFlags::RST) {
                    self.state = TcpState::Closed;
                    self.rx_closed = true;
                    return true;
                }

                // --- ACK processing ---
                if flags.contains(TcpFlags::ACK) {
                    self.process_ack(ack, wnd);
                }

                // --- Data processing ---
                let data_changed = if !payload.is_empty() {
                    self.process_rx_data(seq, payload)
                } else {
                    false
                };

                // --- FIN processing ---
                if flags.contains(TcpFlags::FIN) {
                    return self.process_fin(seq, flags);
                }

                // ACK-only handling for FIN states
                if flags.contains(TcpFlags::ACK) && !data_changed {
                    if self.state == TcpState::FinWait1 {
                        // Check if our FIN is acked
                        // snd_nxt was advanced past FIN, so if ack >= snd_nxt
                        if ack == self.snd_nxt {
                            self.state = TcpState::FinWait2;
                            return true;
                        }
                    }
                }

                return data_changed;
            }

            TcpState::CloseWait => {
                if flags.contains(TcpFlags::RST) {
                    self.state = TcpState::Closed;
                    return true;
                }
                if flags.contains(TcpFlags::ACK) {
                    self.process_ack(ack, wnd);
                }
            }

            TcpState::Closing => {
                if flags.contains(TcpFlags::ACK) && ack == self.snd_nxt {
                    self.state = TcpState::TimeWait;
                    return true;
                }
            }

            TcpState::LastAck => {
                if flags.contains(TcpFlags::ACK) && ack == self.snd_nxt {
                    self.state = TcpState::Closed;
                    return true;
                }
            }

            _ => {}
        }

        false
    }

    /// Process an incoming ACK: advance snd_una, trim tx_buf, track dup ACKs.
    fn process_ack(&mut self, ack: u32, wnd: u32) {
        self.snd_wnd = wnd;

        if seq_after(ack, self.snd_nxt) {
            // ACK for data we haven't sent — ignore
            return;
        }

        if seq_after(ack, self.snd_una) {
            // New data acknowledged
            let acked = ack.wrapping_sub(self.snd_una) as usize;
            // Drain acknowledged bytes from tx_buf
            let drain = acked.min(self.tx_buf.len());
            self.tx_buf.drain(..drain);
            self.snd_una = ack;
            self.dup_ack_count = 0;
            self.retransmit_count = 0;
        } else if ack == self.snd_una && !self.tx_buf.is_empty() {
            // Duplicate ACK
            self.dup_ack_count += 1;
            if self.dup_ack_count >= DUP_ACK_THRESHOLD {
                // Fast retransmit
                let _ = self.retransmit();
                self.dup_ack_count = 0;
            }
        }
    }

    /// Process received data segment. Returns true if new data was buffered.
    fn process_rx_data(&mut self, seq: u32, payload: &[u8]) -> bool {
        let seg_end = seq.wrapping_add(payload.len() as u32);

        if seq == self.rcv_nxt {
            // In-order: append directly to rx_buf
            self.rx_buf.extend(payload);
            self.rcv_nxt = seg_end;
            // Try to merge any buffered out-of-order segments
            self.merge_ooo();
            let _ = self.send_segment(TcpFlags::ACK, &[]);
            true
        } else if seq_after(seg_end, self.rcv_nxt) {
            // Out-of-order or partially overlapping: buffer it
            // Only store if within receive window
            let ahead = seq.wrapping_sub(self.rcv_nxt) as usize;
            if ahead <= DEFAULT_WINDOW as usize {
                self.ooo_segs.entry(seq).or_insert_with(|| payload.to_vec());
                // Send duplicate ACK to trigger fast retransmit at sender
                let _ = self.send_segment(TcpFlags::ACK, &[]);
            }
            false
        } else {
            // Entirely old/duplicate data, ignore
            let _ = self.send_segment(TcpFlags::ACK, &[]);
            false
        }
    }

    /// Process a FIN flag. Returns true.
    fn process_fin(&mut self, seq: u32, flags: TcpFlags) -> bool {
        // FIN occupies one sequence number, positioned after any payload
        // Only process if we've received all data up to the FIN
        if seq_after(seq, self.rcv_nxt) && !flags.contains(TcpFlags::ACK) {
            // Not yet ready to process this FIN (gap exists)
            return false;
        }
        self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
        let _ = self.send_segment(TcpFlags::ACK, &[]);

        match self.state {
            TcpState::Established => {
                self.state = TcpState::CloseWait;
                self.rx_closed = true;
            }
            TcpState::FinWait1 => {
                if flags.contains(TcpFlags::ACK) {
                    self.state = TcpState::TimeWait;
                } else {
                    self.state = TcpState::Closing;
                }
                self.rx_closed = true;
            }
            TcpState::FinWait2 => {
                self.state = TcpState::TimeWait;
                self.rx_closed = true;
            }
            _ => {}
        }
        true
    }

    /// Drive the receive loop: drain all available segments from the interface.
    fn drain_segments(&mut self) {
        let local = match self.local {
            Some(l) => l,
            None => return,
        };
        let iface = match &self.iface {
            Some(i) => i.clone(),
            None => return,
        };

        while let Some((src, data)) = iface.try_recv_tcp(local.port) {
            self.process_segment(src, &data);
        }
    }

    /// Process segments from the listen port and enqueue completed connections.
    fn process_listen_segment(
        iface: &Arc<Interface>,
        local: SocketAddr,
        src: SocketAddr,
        data: &[u8],
        accept_queue: &Arc<SpinLock<AcceptQueue>>,
    ) {
        let pkt = match TCPPacket::parse(data) {
            Some(p) => p,
            None => return,
        };

        let flags = pkt.flags();

        if flags.contains(TcpFlags::SYN) && !flags.contains(TcpFlags::ACK) {
            let mut q = accept_queue.lock();
            if q.queue.len() >= q.capacity {
                return; // backlog full, drop
            }

            let remote = SocketAddr::new(src.ip, pkt.src_port());
            let local_seq = initial_seq();
            let rcv_nxt = pkt.seq_num().wrapping_add(1);

            // Send SYN-ACK
            let _ = iface.send_tcp(
                local,
                remote,
                local_seq,
                rcv_nxt,
                TcpFlags::SYN | TcpFlags::ACK,
                DEFAULT_WINDOW,
                &[],
            );

            // For simplicity, treat as completed immediately.
            // A proper implementation would wait for the ACK in SynReceived state.
            q.queue.push_back(PendingConn {
                local,
                remote,
                snd_nxt: local_seq.wrapping_add(1), // SYN consumes one seq
                rcv_nxt,
                iface: iface.clone(),
            });
        }
    }
}

impl SocketInner for TcpInner {
    fn bind(&mut self, addr: SocketAddr) -> SysResult<()> {
        if self.local.is_some() {
            return Err(Errno::EINVAL);
        }
        if self.state != TcpState::Closed {
            return Err(Errno::EINVAL);
        }
        if addr.port != 0 && addr.port < 1024 && current::euid() != 0 {
            return Err(Errno::EACCES);
        }

        let iface = if addr.ip.is_unspecified() {
            manager::default_interface().ok_or(Errno::EADDRNOTAVAIL)?
        } else {
            manager::find_interface_for(addr.ip).ok_or(Errno::EADDRNOTAVAIL)?
        };

        iface.bind_tcp(addr.port);
        self.local = Some(addr);
        self.iface = Some(iface);
        Ok(())
    }

    fn connect(&mut self, addr: SocketAddr, blocked: bool) -> SysResult<()> {
        if self.state != TcpState::Closed {
            return Err(Errno::EISCONN);
        }

        // Auto-bind if needed
        if self.local.is_none() {
            let iface = self.resolve_iface()?;
            let port = iface.alloc_ephemeral_tcp_port();
            iface.bind_tcp(port);
            self.local = Some(SocketAddr::any(port));
        }

        self.remote = Some(addr);
        let isn = initial_seq();
        self.snd_una = isn;
        self.snd_nxt = isn;

        // Send SYN
        self.state = TcpState::SynSent;
        self.send_segment(TcpFlags::SYN, &[])?;
        self.snd_nxt = self.snd_nxt.wrapping_add(1); // SYN consumes one seq

        if !blocked {
            return Err(Errno::EINPROGRESS);
        }

        // Wait for SYN-ACK
        loop {
            let (src, data) = self.recv_segment(true)?;
            self.process_segment(src, &data);

            match self.state {
                TcpState::Established => return Ok(()),
                TcpState::Closed => return Err(Errno::ECONNREFUSED),
                _ => continue,
            }
        }
    }

    fn listen(&mut self, backlog: usize) -> SysResult<()> {
        if self.local.is_none() {
            return Err(Errno::EINVAL);
        }
        if self.state != TcpState::Closed {
            return Err(Errno::EINVAL);
        }

        let capacity = backlog.max(1);
        self.backlog = Some(Arc::new(SpinLock::new(
            AcceptQueue {
                queue: VecDeque::new(),
                capacity,
            },
            "TcpInner::backlog",
        )));
        self.state = TcpState::Listen;
        Ok(())
    }

    fn accept(&mut self, blocked: bool) -> SysResult<Arc<InetSocket>> {
        if self.state != TcpState::Listen {
            return Err(Errno::EINVAL);
        }

        let local = self.local.ok_or(Errno::EINVAL)?;
        let iface = self.iface.as_ref().ok_or(Errno::EINVAL)?.clone();
        let accept_queue = self.backlog.as_ref().ok_or(Errno::EINVAL)?.clone();

        loop {
            {
                let mut q = accept_queue.lock();
                if let Some(conn) = q.queue.pop_front() {
                    let inner = TcpInner::from_accepted(conn);
                    let sock = InetSocket::from_inner(Box::new(inner), blocked);
                    return Ok(Arc::new(sock));
                }
            }

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            let (src, data) = self.recv_segment(true)?;
            TcpInner::process_listen_segment(&iface, local, src, &data, &accept_queue);
        }
    }

    fn sendto(&mut self, buf: &[u8], dst: Option<SocketAddr>, _blocked: bool) -> SysResult<usize> {
        if dst.is_some() {
            return Err(Errno::EISCONN);
        }
        if self.tx_closed {
            return Err(Errno::EPIPE);
        }

        match self.state {
            TcpState::Established | TcpState::CloseWait => {}
            _ => return Err(Errno::ENOTCONN),
        }

        // Drain incoming ACKs first to open up send window
        self.drain_segments();

        let mut sent = 0;
        while sent < buf.len() {
            let window = self.send_window_available();
            if window == 0 {
                // Window full — drain ACKs and retry
                self.drain_segments();
                let window = self.send_window_available();
                if window == 0 {
                    // Still full; wait for an ACK
                    if let Ok((src, data)) = self.recv_segment(true) {
                        self.process_segment(src, &data);
                    }
                    continue;
                }
            }

            let chunk_len = buf[sent..].len().min(MSS).min(self.send_window_available());
            if chunk_len == 0 {
                continue;
            }
            let chunk = &buf[sent..sent + chunk_len];

            let is_last = sent + chunk_len == buf.len();
            let flags = if is_last {
                TcpFlags::PSH | TcpFlags::ACK
            } else {
                TcpFlags::ACK
            };
            self.send_segment(flags, chunk)?;

            // Buffer for retransmission
            self.tx_buf.extend(chunk);
            self.snd_nxt = self.snd_nxt.wrapping_add(chunk_len as u32);
            sent += chunk_len;
        }

        // Drain any incoming ACKs
        self.drain_segments();

        Ok(sent)
    }

    fn recvfrom(&mut self, buf: &mut [u8], blocked: bool) -> SysResult<(usize, Option<SocketAddr>)> {
        let remote = self.remote;

        loop {
            self.drain_segments();

            if !self.rx_buf.is_empty() {
                let n = buf.len().min(self.rx_buf.len());
                for i in 0..n {
                    buf[i] = self.rx_buf.pop_front().unwrap();
                }
                return Ok((n, remote));
            }

            if self.rx_closed {
                return Ok((0, remote));
            }

            match self.state {
                TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2 => {}
                _ => return Ok((0, remote)),
            }

            if !blocked {
                return Err(Errno::EAGAIN);
            }

            let (src, data) = self.recv_segment(true)?;
            self.process_segment(src, &data);
        }
    }

    fn shutdown(&mut self, how: usize) -> SysResult<()> {
        if how == SHUT_WR || how == SHUT_RDWR {
            if !self.tx_closed {
                self.tx_closed = true;
                match self.state {
                    TcpState::Established => {
                        let _ = self.send_segment(TcpFlags::FIN | TcpFlags::ACK, &[]);
                        self.snd_nxt = self.snd_nxt.wrapping_add(1);
                        self.state = TcpState::FinWait1;
                    }
                    TcpState::CloseWait => {
                        let _ = self.send_segment(TcpFlags::FIN | TcpFlags::ACK, &[]);
                        self.snd_nxt = self.snd_nxt.wrapping_add(1);
                        self.state = TcpState::LastAck;
                    }
                    _ => {}
                }
            }
        }
        if how == SHUT_RDWR {
            self.rx_closed = true;
        }
        Ok(())
    }

    fn poll_read(&self) -> bool {
        if !self.rx_buf.is_empty() || self.rx_closed {
            return true;
        }
        if self.state == TcpState::Listen {
            if let Some(ref q) = self.backlog {
                return !q.lock().queue.is_empty();
            }
        }
        if let Some(ref iface) = self.iface {
            if let Some(local) = self.local {
                return iface.has_tcp_data(local.port);
            }
        }
        false
    }

    fn poll_write(&self) -> bool {
        matches!(self.state, TcpState::Established | TcpState::CloseWait)
            && !self.tx_closed
            && self.send_window_available() > 0
    }

    fn type_name(&self) -> &'static str {
        "inet-tcp"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local
    }
}

impl Drop for TcpInner {
    fn drop(&mut self) {
        match self.state {
            TcpState::Established | TcpState::CloseWait => {
                let _ = self.send_segment(TcpFlags::FIN | TcpFlags::ACK, &[]);
            }
            TcpState::SynSent | TcpState::SynReceived => {
                self.send_rst();
            }
            _ => {}
        }
        if let (Some(local), Some(iface)) = (self.local.take(), self.iface.take()) {
            if self.backlog.is_none() || self.state == TcpState::Listen {
                iface.unbind_tcp(local.port);
            }
        }
    }
}

fn seq_after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

fn seq_before_eq(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}

fn initial_seq() -> u32 {
    static COUNTER: AtomicU32 = AtomicU32::new(1000);
    COUNTER.fetch_add(64000, Ordering::Relaxed)
}
