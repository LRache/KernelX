mod arp;
mod dhcp;
mod dispatch;
mod driver;
mod port;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use core::net::Ipv4Addr;

use crate::driver::NetDriverOps;
use crate::kernel::errno::SysResult;
use crate::klib::SpinLock;
use crate::net::protocol::tcp::TcpFlags;
use crate::net::protocol::{
    EthernetBuilder, IPv4Builder, MacAddr, ProtocolBuilder, RawPayload, TCPBuilder, UDPBuilder,
};
use crate::net::socket::SocketAddr;

use arp::ArpTable;
use port::PortMap;

/// A network interface. Wraps an optional hardware driver.
/// When `driver` is `None`, the interface behaves as loopback.
pub struct Interface {
    pub name: String,
    pub driver: Option<Arc<dyn NetDriverOps>>,
    ipv4: SpinLock<Option<Ipv4Addr>>,
    netmask: SpinLock<Option<Ipv4Addr>>,
    gateway: SpinLock<Option<Ipv4Addr>>,
    pub udp_rx: SpinLock<PortMap>,
    pub tcp_rx: SpinLock<PortMap>,
    pub raw_rx: SpinLock<PortMap>,
    arp_table: SpinLock<ArpTable>,
}

impl Interface {
    pub fn new(name: String, driver: Arc<dyn NetDriverOps>) -> Self {
        Self {
            name,
            driver: Some(driver),
            ipv4: SpinLock::new(None, "Interface::ipv4"),
            netmask: SpinLock::new(None, "Interface::netmask"),
            gateway: SpinLock::new(None, "Interface::gateway"),
            udp_rx: SpinLock::new(PortMap::new(), "Interface::udp_rx"),
            tcp_rx: SpinLock::new(PortMap::new(), "Interface::tcp_rx"),
            raw_rx: SpinLock::new(PortMap::new(), "Interface::raw_rx"),
            arp_table: SpinLock::new(ArpTable::new(), "Interface::arp_table"),
        }
    }

    pub fn loopback() -> Self {
        Self {
            name: "lo".into(),
            driver: None,
            ipv4: SpinLock::new(Some(Ipv4Addr::LOCALHOST), "Interface::ipv4"),
            netmask: SpinLock::new(Some(Ipv4Addr::new(255, 0, 0, 0)), "Interface::netmask"),
            gateway: SpinLock::new(None, "Interface::gateway"),
            udp_rx: SpinLock::new(PortMap::new(), "Interface::udp_rx"),
            tcp_rx: SpinLock::new(PortMap::new(), "Interface::tcp_rx"),
            raw_rx: SpinLock::new(PortMap::new(), "Interface::raw_rx"),
            arp_table: SpinLock::new(ArpTable::new(), "Interface::arp_table"),
        }
    }

    pub fn is_loopback(&self) -> bool {
        self.driver.is_none()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn driver(&self) -> Option<&Arc<dyn NetDriverOps>> {
        self.driver.as_ref()
    }

    pub fn set_ipv4(&self, ip: Ipv4Addr, mask: Ipv4Addr, gw: Ipv4Addr) {
        *self.ipv4.lock() = Some(ip);
        *self.netmask.lock() = Some(mask);
        *self.gateway.lock() = Some(gw);
    }

    pub fn ipv4(&self) -> Option<Ipv4Addr> {
        *self.ipv4.lock()
    }

    pub fn netmask(&self) -> Option<Ipv4Addr> {
        *self.netmask.lock()
    }

    pub fn gateway(&self) -> Option<Ipv4Addr> {
        *self.gateway.lock()
    }

    pub fn send_packet(&self, packet: &[u8]) -> SysResult<()> {
        match &self.driver {
            Some(drv) => drv.send_packet(packet),
            None => {
                self.on_receive(packet);
                Ok(())
            }
        }
    }

    pub fn mac_address(&self) -> MacAddr {
        match &self.driver {
            Some(drv) => drv.mac_address(),
            None => MacAddr::UNSPECIFIED,
        }
    }

    pub fn mtu(&self) -> usize {
        match &self.driver {
            Some(drv) => drv.mtu(),
            None => 65536,
        }
    }

    /// Resolve source IP: use interface IP if unspecified.
    fn resolve_src_ip(&self, src_ip: Ipv4Addr) -> Ipv4Addr {
        if src_ip.is_unspecified() {
            self.ipv4().unwrap_or(Ipv4Addr::UNSPECIFIED)
        } else {
            src_ip
        }
    }

    /// Build and send a UDP packet.
    pub fn send_udp(&self, src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> SysResult<()> {
        let dst_mac = self.resolve_mac(dst.ip)?;
        let raw = RawPayload(payload);
        let udp = UDPBuilder::new().src_port(src.port).dst_port(dst.port).payload(&raw);
        let ipv4 = IPv4Builder::new()
            .src_ip(self.resolve_src_ip(src.ip))
            .dst_ip(dst.ip)
            .udp(udp);
        let eth = EthernetBuilder::new()
            .src_mac(self.mac_address())
            .dst_mac(dst_mac)
            .ipv4(ipv4);

        let len = eth.len();
        let mut buf = vec![0u8; len];
        eth.build(&mut buf)?;
        self.send_packet(&buf)
    }

    /// Build and send a TCP segment.
    pub fn send_tcp(
        &self,
        src: SocketAddr,
        dst: SocketAddr,
        seq: u32,
        ack: u32,
        flags: TcpFlags,
        window: u16,
        payload: &[u8],
    ) -> SysResult<()> {
        let dst_mac = self.resolve_mac(dst.ip)?;
        let raw = RawPayload(payload);
        let mut tcp = TCPBuilder::new(src.port, dst.port)
            .seq_num(seq)
            .ack_num(ack)
            .flags(flags)
            .window_size(window);
        if !payload.is_empty() {
            tcp = tcp.payload(&raw);
        }
        let ipv4 = IPv4Builder::new()
            .src_ip(self.resolve_src_ip(src.ip))
            .dst_ip(dst.ip)
            .tcp(tcp);
        let eth = EthernetBuilder::new()
            .src_mac(self.mac_address())
            .dst_mac(dst_mac)
            .ipv4(ipv4);

        let len = eth.len();
        let mut buf = vec![0u8; len];
        eth.build(&mut buf)?;
        self.send_packet(&buf)
    }

    /// Build and send an IPv4 raw payload with explicit protocol number.
    pub fn send_raw(&self, src_ip: Ipv4Addr, dst_ip: Ipv4Addr, protocol: u8, payload: &[u8]) -> SysResult<()> {
        let dst_mac = self.resolve_mac(dst_ip)?;
        let ipv4 = IPv4Builder::new()
            .src_ip(self.resolve_src_ip(src_ip))
            .dst_ip(dst_ip)
            .raw(protocol, payload);
        let eth = EthernetBuilder::new()
            .src_mac(self.mac_address())
            .dst_mac(dst_mac)
            .ipv4(ipv4);

        crate::kinfo!(
            "send_raw: src_ip={}, dst_ip={}, protocol={:#04x}, payload_len={}",
            src_ip,
            dst_ip,
            protocol,
            payload.len()
        );

        let len = eth.len();
        let mut buf = vec![0u8; len];
        eth.build(&mut buf)?;
        self.send_packet(&buf)
    }
}
