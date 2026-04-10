use alloc::collections::VecDeque;
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

use crate::kernel::event::{Event, WaitQueue};
use crate::kernel::scheduler::current;
use crate::net::socket::SocketAddr;

use super::Interface;

/// A packet queue with integrated wait queue.
struct PacketQueue {
    packets: VecDeque<(SocketAddr, Vec<u8>)>,
    waiters: WaitQueue<()>,
}

impl PacketQueue {
    fn new() -> Self {
        Self {
            packets: VecDeque::new(),
            waiters: WaitQueue::new(),
        }
    }

    fn push(&mut self, src: SocketAddr, packet: Vec<u8>) {
        self.packets.push_back((src, packet));
        self.waiters.wake_all(|_| Event::ReadReady);
    }

    fn has_data(&self) -> bool {
        !self.packets.is_empty()
    }
}

/// Per-port packet dispatch table for a transport protocol.
pub(super) struct PortMap {
    ports: BTreeMap<u16, PacketQueue>,
}

impl PortMap {
    pub(super) fn new() -> Self {
        Self { ports: BTreeMap::new() }
    }

    /// Dispatch a packet to the queue for `port`. Drops if no one is listening.
    pub(super) fn dispatch(&mut self, port: u16, src: SocketAddr, payload: Vec<u8>) {
        if let Some(q) = self.ports.get_mut(&port) {
            q.push(src, payload);
        }
    }

    pub(super) fn bind(&mut self, port: u16) -> &mut PacketQueue {
        self.ports.entry(port).or_insert_with(PacketQueue::new)
    }

    pub(super) fn unbind(&mut self, port: u16) {
        self.ports.remove(&port);
    }

    pub(super) fn has_data(&self, port: u16) -> bool {
        self.ports.get(&port).map_or(false, |q| q.has_data())
    }
}

// ---- Interface methods for UDP ----

impl Interface {
    pub fn bind_udp(&self, port: u16) {
        self.udp_rx.lock().bind(port);
    }

    pub fn unbind_udp(&self, port: u16) {
        self.udp_rx.lock().unbind(port);
    }

    /// Blocking receive. Returns (source_addr, payload). Used by DHCP and legacy code.
    pub fn recv_udp(&self, port: u16) -> Vec<u8> {
        loop {
            {
                let mut map = self.udp_rx.lock();
                let q = map.bind(port);
                if let Some((_src, pkt)) = q.packets.pop_front() {
                    return pkt;
                }
                q.waiters.wait_current(());
            }
            current::schedule();
        }
    }

    /// Non-blocking try receive for socket layer.
    pub fn try_recv_udp(&self, port: u16) -> Option<(SocketAddr, Vec<u8>)> {
        let mut map = self.udp_rx.lock();
        let q = map.ports.get_mut(&port)?;
        q.packets.pop_front()
    }

    /// Check if there's data available on a UDP port.
    pub fn has_udp_data(&self, port: u16) -> bool {
        self.udp_rx.lock().has_data(port)
    }

    /// Block current task on the UDP port's wait queue.
    pub fn wait_udp(&self, port: u16) {
        let mut map = self.udp_rx.lock();
        let q = map.bind(port);
        q.waiters.wait_current(());
    }

    /// Allocate an ephemeral port for UDP.
    pub fn alloc_ephemeral_udp_port(&self) -> u16 {
        static NEXT_PORT: AtomicU16 = AtomicU16::new(49152);
        NEXT_PORT.fetch_add(1, Ordering::Relaxed)
    }

    /// Allocate an ephemeral port for TCP.
    pub fn alloc_ephemeral_tcp_port(&self) -> u16 {
        static NEXT_PORT: AtomicU16 = AtomicU16::new(49152);
        NEXT_PORT.fetch_add(1, Ordering::Relaxed)
    }
}

// ---- Interface methods for TCP ----

impl Interface {
    pub fn bind_tcp(&self, port: u16) {
        self.tcp_rx.lock().bind(port);
    }

    pub fn unbind_tcp(&self, port: u16) {
        self.tcp_rx.lock().unbind(port);
    }

    /// Blocking receive of a raw TCP segment. Used by TCP socket layer.
    pub fn recv_tcp_raw(&self, port: u16) -> (SocketAddr, Vec<u8>) {
        loop {
            {
                let mut map = self.tcp_rx.lock();
                let q = map.bind(port);
                if let Some(entry) = q.packets.pop_front() {
                    return entry;
                }
                q.waiters.wait_current(());
            }
            current::schedule();
        }
    }

    /// Non-blocking try receive for TCP socket layer.
    pub fn try_recv_tcp(&self, port: u16) -> Option<(SocketAddr, Vec<u8>)> {
        let mut map = self.tcp_rx.lock();
        let q = map.ports.get_mut(&port)?;
        q.packets.pop_front()
    }

    /// Check if there's data available on a TCP port.
    pub fn has_tcp_data(&self, port: u16) -> bool {
        self.tcp_rx.lock().has_data(port)
    }

    /// Block current task on the TCP port's wait queue.
    pub fn wait_tcp(&self, port: u16) {
        let mut map = self.tcp_rx.lock();
        let q = map.bind(port);
        q.waiters.wait_current(());
    }
}
