use alloc::collections::VecDeque;
use alloc::collections::btree_map::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

use crate::kernel::event::{EpollNotifier, Event, FileEvent, WaitQueue};
use crate::kernel::scheduler::current;
use crate::net::socket::SocketAddr;

use super::Interface;

/// A packet queue with integrated wait queue.
struct PacketQueue {
    packets: VecDeque<(SocketAddr, Vec<u8>)>,
    waiters: WaitQueue<Event>,
    notifier: Arc<EpollNotifier>,
    bind_count: usize,
}

impl PacketQueue {
    fn new() -> Self {
        Self {
            packets: VecDeque::new(),
            waiters: WaitQueue::new("PacketQueue::waiters"),
            notifier: Arc::new(EpollNotifier::new()),
            bind_count: 0,
        }
    }

    fn push(&mut self, src: SocketAddr, packet: Vec<u8>) {
        self.push_with_notify(src, packet, true);
    }

    fn push_with_notify(&mut self, src: SocketAddr, packet: Vec<u8>, notify: bool) {
        self.packets.push_back((src, packet));
        if notify {
            self.waiters.wake_all(|event| event);
            self.notifier.notify(FileEvent::READ_READY);
        }
    }

    fn push_wake_read_waiters(&mut self, src: SocketAddr, packet: Vec<u8>) {
        self.packets.push_back((src, packet));
        self.waiters
            .wake_all_by(|event| matches!(event, Event::ReadReady), |event| event);
    }

    fn has_data(&self) -> bool {
        !self.packets.is_empty()
    }
}

/// Per-port packet dispatch table for a transport protocol.
pub struct PortMap {
    ports: BTreeMap<u16, PacketQueue>,
    flows: BTreeMap<(u16, SocketAddr), PacketQueue>,
}

impl PortMap {
    pub(super) fn new() -> Self {
        Self {
            ports: BTreeMap::new(),
            flows: BTreeMap::new(),
        }
    }

    /// Dispatch a packet to the queue for `port`. Drops if no one is listening.
    pub(super) fn dispatch(&mut self, port: u16, src: SocketAddr, payload: Vec<u8>) {
        let payload_len = payload.len();
        if let Some(q) = self.ports.get_mut(&port) {
            q.push(src, payload);
        } else {
            crate::kwarn!(
                "net port drop: no listener port={} src={}:{} payload_len={}",
                port,
                src.ip,
                src.port,
                payload_len
            );
        }
    }

    pub(super) fn dispatch_silent(&mut self, port: u16, src: SocketAddr, payload: Vec<u8>) {
        if let Some(q) = self.ports.get_mut(&port) {
            q.push(src, payload);
        }
    }

    pub(super) fn dispatch_flow_or_port(
        &mut self,
        port: u16,
        src: SocketAddr,
        payload: Vec<u8>,
        notify_flow: bool,
        fallback_to_port: bool,
    ) {
        let port_notifier = if notify_flow {
            self.ports.get(&port).map(|q| q.notifier.clone())
        } else {
            None
        };
        if let Some(q) = self.flows.get_mut(&(port, src)) {
            if notify_flow {
                q.push_with_notify(src, payload, true);
            } else {
                q.push_wake_read_waiters(src, payload);
            }
            if let Some(notifier) = port_notifier.as_ref() {
                notifier.notify(FileEvent::READ_READY);
            }
        } else if fallback_to_port {
            self.dispatch(port, src, payload);
        }
    }

    fn queue_mut(&mut self, port: u16) -> &mut PacketQueue {
        self.ports.entry(port).or_insert_with(PacketQueue::new)
    }

    fn bind(&mut self, port: u16) {
        let q = self.queue_mut(port);
        q.bind_count += 1;
    }

    fn flow_queue_mut(&mut self, port: u16, remote: SocketAddr) -> &mut PacketQueue {
        let mut pending = Vec::new();
        if let Some(q) = self.ports.get_mut(&port) {
            let mut i = 0;
            while i < q.packets.len() {
                if q.packets[i].0 == remote {
                    if let Some(entry) = q.packets.remove(i) {
                        pending.push(entry);
                    }
                } else {
                    i += 1;
                }
            }
        }

        let q = self.flows.entry((port, remote)).or_insert_with(PacketQueue::new);
        for (src, packet) in pending {
            q.push(src, packet);
        }
        q
    }

    fn bind_flow(&mut self, port: u16, remote: SocketAddr) {
        let q = self.flow_queue_mut(port, remote);
        q.bind_count += 1;
    }

    pub(super) fn unbind(&mut self, port: u16) {
        let should_remove = if let Some(q) = self.ports.get_mut(&port) {
            q.bind_count = q.bind_count.saturating_sub(1);
            q.bind_count == 0
        } else {
            false
        };
        if should_remove {
            self.ports.remove(&port);
        }
    }

    pub(super) fn unbind_flow(&mut self, port: u16, remote: SocketAddr) {
        let should_remove = if let Some(q) = self.flows.get_mut(&(port, remote)) {
            q.bind_count = q.bind_count.saturating_sub(1);
            q.bind_count == 0
        } else {
            false
        };
        if should_remove {
            self.flows.remove(&(port, remote));
        }
    }

    pub(super) fn has_data(&self, port: u16) -> bool {
        self.ports.get(&port).map_or(false, |q| q.has_data())
    }

    pub(super) fn has_flow_data(&self, port: u16, remote: SocketAddr) -> bool {
        self.flows.get(&(port, remote)).map_or(false, |q| q.has_data())
    }

    pub(super) fn cancel_wait_current(&mut self, port: u16) {
        if let Some(q) = self.ports.get_mut(&port) {
            q.waiters.remove_current();
        }
    }

    pub(super) fn cancel_wait_flow_current(&mut self, port: u16, remote: SocketAddr) {
        if let Some(q) = self.flows.get_mut(&(port, remote)) {
            q.waiters.remove_current();
        }
    }

    pub(super) fn wait_current(&mut self, port: u16, event: Event) -> bool {
        let q = self.queue_mut(port);
        if q.has_data() {
            return true;
        }
        q.waiters.wait_current(event);
        false
    }

    pub(super) fn wait_flow_current(&mut self, port: u16, remote: SocketAddr, event: Event) -> bool {
        let q = self.flow_queue_mut(port, remote);
        if q.has_data() {
            return true;
        }
        q.waiters.wait_current(event);
        false
    }

    pub(super) fn wait_event(&mut self, port: u16, event: Event) -> bool {
        let q = self.queue_mut(port);
        if q.has_data() {
            return true;
        }
        q.waiters.wait_pending(current::task().clone(), event);
        false
    }

    pub(super) fn wait_flow_event(&mut self, port: u16, remote: SocketAddr, event: Event) -> bool {
        let q = self.flow_queue_mut(port, remote);
        if q.has_data() {
            return true;
        }
        q.waiters.wait_pending(current::task().clone(), event);
        false
    }

    pub(super) fn epoll_notifier(&mut self, port: u16) -> Arc<EpollNotifier> {
        self.queue_mut(port).notifier.clone()
    }

    pub(super) fn flow_epoll_notifier(&mut self, port: u16, remote: SocketAddr) -> Arc<EpollNotifier> {
        self.flow_queue_mut(port, remote).notifier.clone()
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

    pub fn bind_udp_flow(&self, port: u16, remote: SocketAddr) {
        self.udp_rx.lock().bind_flow(port, remote);
    }

    pub fn unbind_udp_flow(&self, port: u16, remote: SocketAddr) {
        self.udp_rx.lock().unbind_flow(port, remote);
    }

    /// Blocking receive. Returns (source_addr, payload). Used by DHCP and legacy code.
    pub fn recv_udp(&self, port: u16) -> Vec<u8> {
        loop {
            let ready = {
                let mut map = self.udp_rx.lock();
                let q = map.queue_mut(port);
                if let Some((_src, pkt)) = q.packets.pop_front() {
                    return pkt;
                }
                q.has_data()
            };
            if ready || self.wait_udp(port) {
                continue;
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

    pub fn try_recv_udp_flow(&self, port: u16, remote: SocketAddr) -> Option<(SocketAddr, Vec<u8>)> {
        let mut map = self.udp_rx.lock();
        let q = map.flows.get_mut(&(port, remote))?;
        q.packets.pop_front()
    }

    /// Check if there's data available on a UDP port.
    pub fn has_udp_data(&self, port: u16) -> bool {
        self.udp_rx.lock().has_data(port)
    }

    pub fn has_udp_flow_data(&self, port: u16, remote: SocketAddr) -> bool {
        self.udp_rx.lock().has_flow_data(port, remote)
    }

    /// Block current task on the UDP port's wait queue.
    pub fn wait_udp(&self, port: u16) -> bool {
        self.udp_rx.lock().wait_current(port, Event::ReadReady)
    }

    pub fn wait_udp_flow(&self, port: u16, remote: SocketAddr) -> bool {
        self.udp_rx.lock().wait_flow_current(port, remote, Event::ReadReady)
    }

    pub fn wait_udp_poll(&self, port: u16, waker: usize) -> bool {
        self.udp_rx.lock().wait_event(
            port,
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        )
    }

    pub fn wait_udp_flow_poll(&self, port: u16, remote: SocketAddr, waker: usize) -> bool {
        self.udp_rx.lock().wait_flow_event(
            port,
            remote,
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        )
    }

    pub fn cancel_wait_udp(&self, port: u16) {
        self.udp_rx.lock().cancel_wait_current(port);
    }

    pub fn cancel_wait_udp_flow(&self, port: u16, remote: SocketAddr) {
        self.udp_rx.lock().cancel_wait_flow_current(port, remote);
    }

    pub fn udp_epoll_notifier(&self, port: u16) -> Arc<EpollNotifier> {
        self.udp_rx.lock().epoll_notifier(port)
    }

    pub fn udp_flow_epoll_notifier(&self, port: u16, remote: SocketAddr) -> Arc<EpollNotifier> {
        self.udp_rx.lock().flow_epoll_notifier(port, remote)
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

    pub fn bind_tcp_flow(&self, port: u16, remote: SocketAddr) {
        self.tcp_rx.lock().bind_flow(port, remote);
    }

    pub fn unbind_tcp_flow(&self, port: u16, remote: SocketAddr) {
        self.tcp_rx.lock().unbind_flow(port, remote);
    }

    /// Blocking receive of a raw TCP segment. Used by TCP socket layer.
    pub fn recv_tcp_raw(&self, port: u16) -> (SocketAddr, Vec<u8>) {
        loop {
            {
                let mut map = self.tcp_rx.lock();
                let q = map.queue_mut(port);
                if let Some(entry) = q.packets.pop_front() {
                    return entry;
                }
                q.waiters.wait_current(Event::ReadReady);
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

    pub fn try_recv_tcp_flow(&self, port: u16, remote: SocketAddr) -> Option<(SocketAddr, Vec<u8>)> {
        let mut map = self.tcp_rx.lock();
        let q = map.flows.get_mut(&(port, remote))?;
        q.packets.pop_front()
    }

    /// Block current task on the TCP port's wait queue.
    pub fn wait_tcp(&self, port: u16) -> bool {
        self.tcp_rx.lock().wait_current(port, Event::ReadReady)
    }

    pub fn wait_tcp_flow(&self, port: u16, remote: SocketAddr) -> bool {
        self.tcp_rx.lock().wait_flow_current(port, remote, Event::ReadReady)
    }

    pub fn wait_tcp_poll(&self, port: u16, waker: usize) -> bool {
        self.tcp_rx.lock().wait_event(
            port,
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        )
    }

    pub fn wait_tcp_flow_poll(&self, port: u16, remote: SocketAddr, waker: usize) -> bool {
        self.tcp_rx.lock().wait_flow_event(
            port,
            remote,
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        )
    }

    pub fn cancel_wait_tcp(&self, port: u16) {
        self.tcp_rx.lock().cancel_wait_current(port);
    }

    pub fn cancel_wait_tcp_flow(&self, port: u16, remote: SocketAddr) {
        self.tcp_rx.lock().cancel_wait_flow_current(port, remote);
    }

    pub fn tcp_epoll_notifier(&self, port: u16) -> Arc<EpollNotifier> {
        self.tcp_rx.lock().epoll_notifier(port)
    }

    pub fn tcp_flow_epoll_notifier(&self, port: u16, remote: SocketAddr) -> Arc<EpollNotifier> {
        self.tcp_rx.lock().flow_epoll_notifier(port, remote)
    }
}

// ---- Interface methods for RAW IPv4 protocol dispatch ----

impl Interface {
    pub fn bind_raw(&self, protocol: u8) {
        self.raw_rx.lock().bind(protocol as u16);
    }

    pub fn unbind_raw(&self, protocol: u8) {
        self.raw_rx.lock().unbind(protocol as u16);
    }

    pub fn try_recv_raw(&self, protocol: u8) -> Option<(SocketAddr, Vec<u8>)> {
        let mut map = self.raw_rx.lock();
        let q = map.ports.get_mut(&(protocol as u16))?;
        q.packets.pop_front()
    }

    pub fn has_raw_data(&self, protocol: u8) -> bool {
        self.raw_rx.lock().has_data(protocol as u16)
    }

    pub fn wait_raw(&self, protocol: u8) {
        let mut map = self.raw_rx.lock();
        let q = map.queue_mut(protocol as u16);
        q.waiters.wait_current(Event::ReadReady);
    }

    pub fn wait_raw_poll(&self, protocol: u8, waker: usize) -> bool {
        self.raw_rx.lock().wait_event(
            protocol as u16,
            Event::Poll {
                event: FileEvent::READ_READY,
                waker,
            },
        )
    }

    pub fn cancel_wait_raw(&self, protocol: u8) {
        self.raw_rx.lock().cancel_wait_current(protocol as u16);
    }

    pub fn raw_epoll_notifier(&self, protocol: u8) -> Arc<EpollNotifier> {
        self.raw_rx.lock().epoll_notifier(protocol as u16)
    }
}
