use alloc::vec;
use core::net::Ipv4Addr;

use crate::kernel::errno::SysResult;
use crate::{kdebug, kinfo};

use crate::net::protocol::dhcp::{
    DHCP_CLIENT_PORT, DHCP_SERVER_PORT, DHCPBuilder, DHCPPacket, DhcpMessageType, DhcpOp, DhcpReply,
};
use crate::net::protocol::{EthernetBuilder, IPv4Builder, MacAddr, ProtocolBuilder, UDPBuilder};

use super::Interface;

impl Interface {
    /// Run DHCP to obtain an IP address. Blocks until complete.
    pub fn dhcp_configure(&self) -> SysResult<()> {
        let mac = self.mac_address();
        let xid = crate::klib::random::random();

        // Bind DHCP client port before sending so we don't miss replies
        self.bind_udp(DHCP_CLIENT_PORT);

        // Discover
        self.send_packet(
            &self.build_dhcp_packet(
                DHCPBuilder::new()
                    .op(DhcpOp::Request)
                    .xid(xid)
                    .chaddr(mac)
                    .message_type(DhcpMessageType::Discover),
            ),
        )?;
        kdebug!("DHCP: sent Discover on {}", self.name);

        // Wait for Offer
        let offer = self.wait_dhcp_reply(xid, DhcpMessageType::Offer)?;
        kdebug!(
            "DHCP: got Offer: ip={}.{}.{}.{}",
            offer.your_ip[0],
            offer.your_ip[1],
            offer.your_ip[2],
            offer.your_ip[3]
        );

        // Request
        self.send_packet(
            &self.build_dhcp_packet(
                DHCPBuilder::new()
                    .op(DhcpOp::Request)
                    .xid(xid)
                    .chaddr(mac)
                    .message_type(DhcpMessageType::Request)
                    .add_option(50, &offer.your_ip)
                    .add_option(54, &offer.server_ip),
            ),
        )?;
        kinfo!("DHCP: sent Request");

        // Wait for Ack
        let ack = self.wait_dhcp_reply(xid, DhcpMessageType::Ack)?;

        self.unbind_udp(DHCP_CLIENT_PORT);

        let ip = Ipv4Addr::from(ack.your_ip);
        let mask = Ipv4Addr::from(ack.subnet_mask.unwrap_or([255, 255, 255, 0]));
        let gw = Ipv4Addr::from(ack.router.unwrap_or([0, 0, 0, 0]));

        self.set_ipv4(ip, mask, gw);
        kinfo!("DHCP: {} configured: ip={} mask={} gateway={}", self.name, ip, mask, gw);

        Ok(())
    }

    fn build_dhcp_packet(&self, dhcp: DHCPBuilder) -> alloc::vec::Vec<u8> {
        let mac = self.mac_address();

        let udp = UDPBuilder::new()
            .src_port(DHCP_CLIENT_PORT)
            .dst_port(DHCP_SERVER_PORT)
            .payload(&dhcp);
        let ipv4 = IPv4Builder::new()
            .src_ip(Ipv4Addr::UNSPECIFIED)
            .dst_ip(Ipv4Addr::BROADCAST)
            .udp(udp);
        let eth = EthernetBuilder::new()
            .src_mac(mac)
            .dst_mac(MacAddr::BROADCAST)
            .ipv4(ipv4);

        let len = eth.len();
        let mut buf = vec![0u8; len];
        let _ = eth.build(&mut buf);
        buf
    }

    fn wait_dhcp_reply(&self, xid: u32, expected: DhcpMessageType) -> SysResult<DhcpReply> {
        loop {
            let data = self.recv_udp(DHCP_CLIENT_PORT);
            if let Some(reply) = Self::try_match_dhcp(&data, xid, expected) {
                return Ok(reply);
            }
            // Not our packet, loop and block again
        }
    }

    fn try_match_dhcp(data: &[u8], xid: u32, expected: DhcpMessageType) -> Option<DhcpReply> {
        let dhcp = DHCPPacket::parse(data)?;
        let reply = dhcp.as_reply()?;
        if reply.xid != xid || reply.msg_type != expected {
            return None;
        }
        Some(reply)
    }
}
