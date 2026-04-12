use crate::net::protocol::{EthernetFrame, EthernetFramePayload, IPv4PacketPayload};
use crate::net::socket::SocketAddr;

use super::Interface;

impl Interface {
    /// Parse a received ethernet frame and dispatch to per-port queues.
    pub fn on_receive(&self, packet: &[u8]) {
        let eth = match EthernetFrame::parse(packet) {
            Some(eth) => eth,
            None => return,
        };

        match eth.payload() {
            EthernetFramePayload::IPv4(ip) => {
                let ip_hdr_len = ip.header_len();
                let ip_total = ip.total_length() as usize;
                if ip_total < ip_hdr_len || packet.len() < 14 + ip_total {
                    return;
                }
                let src_ip = ip.src_ip();
                let ip_payload = &packet[14 + ip_hdr_len..14 + ip_total];
                self.raw_rx
                    .lock()
                    .dispatch(ip.protocol() as u16, SocketAddr::new(src_ip, 0), ip_payload.to_vec());

                match ip.payload() {
                    IPv4PacketPayload::UDP(udp) => {
                        let src = SocketAddr::new(src_ip, udp.src_port());
                        self.udp_rx
                            .lock()
                            .dispatch(udp.dst_port(), src, udp.raw_payload().to_vec());
                    }
                    IPv4PacketPayload::TCP(tcp) => {
                        let src = SocketAddr::new(src_ip, tcp.src_port());
                        // Dispatch full TCP segment (header + payload) for state machine
                        self.tcp_rx.lock().dispatch(tcp.dst_port(), src, ip_payload.to_vec());
                    }
                    IPv4PacketPayload::ICMP(_icmp) => {}
                    IPv4PacketPayload::Raw(_) => {}
                }
            }
            EthernetFramePayload::ARP(arp) => {
                self.handle_arp(&arp);
            }
            EthernetFramePayload::Raw(_) => {}
        }
    }
}
