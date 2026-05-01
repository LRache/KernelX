use alloc::collections::BTreeMap;
use alloc::vec;
use core::net::Ipv4Addr;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{Event, WaitQueue};
use crate::kernel::scheduler::current;
use crate::net::protocol::{ARPBuilder, ARPPacket, ArpOperation, EthernetBuilder, MacAddr, ProtocolBuilder};

use super::Interface;

/// Per-interface ARP table.
pub(super) struct ArpTable {
    cache: BTreeMap<u32, MacAddr>,
    /// Tasks waiting for any ARP resolution to complete.
    waiters: WaitQueue<()>,
}

impl ArpTable {
    pub fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
            waiters: WaitQueue::new(),
        }
    }
}

impl Interface {
    pub fn resolve_mac(&self, dst_ip: Ipv4Addr) -> SysResult<MacAddr> {
        if dst_ip.is_broadcast() {
            return Ok(MacAddr::BROADCAST);
        }

        if self.is_loopback() {
            return Ok(MacAddr::UNSPECIFIED);
        }

        let next_hop = self.next_hop(dst_ip);

        // Fast path: check cache
        if let Some(mac) = self.arp_table.lock().cache.get(&u32::from(next_hop)).copied() {
            return Ok(mac);
        }

        // Slow path: ARP request + wait
        self.send_arp_request(next_hop)?;

        for _ in 0..3 {
            // Register on wait queue and drop lock before scheduling
            {
                let mut table = self.arp_table.lock();
                if let Some(mac) = table.cache.get(&u32::from(next_hop)).copied() {
                    return Ok(mac);
                }
                table.waiters.wait_current(());
                // Lock drops here, task is marked blocked
            }
            current::schedule();

            if matches!(current::task().take_wakeup_event(), Some(Event::Signal)) {
                self.arp_table.lock().waiters.remove_current();
                return Err(Errno::EINTR);
            }

            // Woken up — check cache
            if let Some(mac) = self.arp_table.lock().cache.get(&u32::from(next_hop)).copied() {
                return Ok(mac);
            }

            // Retry request
            self.send_arp_request(next_hop)?;
        }

        self.arp_table
            .lock()
            .cache
            .get(&u32::from(next_hop))
            .copied()
            .ok_or(Errno::ENETUNREACH)
    }

    /// Determine the next-hop: same subnet -> dst directly, otherwise -> gateway.
    fn next_hop(&self, dst_ip: Ipv4Addr) -> Ipv4Addr {
        let my_ip = match self.ipv4() {
            Some(ip) => u32::from(ip),
            None => return dst_ip,
        };
        let mask = match self.netmask() {
            Some(m) => u32::from(m),
            None => return dst_ip,
        };

        if (my_ip & mask) == (u32::from(dst_ip) & mask) {
            dst_ip
        } else {
            self.gateway().unwrap_or(dst_ip)
        }
    }

    /// Send an ARP request for `target_ip`.
    fn send_arp_request(&self, target_ip: Ipv4Addr) -> SysResult<()> {
        let src_mac = self.mac_address();
        let src_ip = self.ipv4().unwrap_or(Ipv4Addr::UNSPECIFIED);

        let arp = ARPBuilder::new(ArpOperation::Request, src_mac, src_ip, MacAddr::UNSPECIFIED, target_ip);
        let eth = EthernetBuilder::new()
            .src_mac(src_mac)
            .dst_mac(MacAddr::BROADCAST)
            .arp(arp);

        let len = eth.len();
        let mut buf = vec![0u8; len];
        eth.build(&mut buf)?;
        self.send_packet(&buf)
    }

    /// Handle an incoming ARP packet. Called from dispatch.
    pub fn handle_arp(&self, arp: &ARPPacket<'_>) {
        let sender_ip = Ipv4Addr::from(arp.sender_ip());
        let sender_mac = arp.sender_mac();

        // Learn from any ARP packet
        {
            let mut table = self.arp_table.lock();
            table.cache.insert(u32::from(sender_ip), sender_mac);
            table.waiters.wake_all(|_| Event::ReadReady);
        }

        // Reply to requests for our IP
        if arp.operation() == ArpOperation::Request {
            let target_ip = Ipv4Addr::from(arp.target_ip());
            if Some(target_ip) == self.ipv4() {
                let my_mac = self.mac_address();
                let reply = ARPBuilder::new(
                    ArpOperation::Reply,
                    my_mac,
                    target_ip,
                    arp.sender_mac(),
                    arp.sender_ip(),
                );
                let eth = EthernetBuilder::new().src_mac(my_mac).dst_mac(sender_mac).arp(reply);

                let len = eth.len();
                let mut buf = vec![0u8; len];
                if eth.build(&mut buf).is_ok() {
                    let _ = self.send_packet(&buf);
                }
            }
        }
    }
}
