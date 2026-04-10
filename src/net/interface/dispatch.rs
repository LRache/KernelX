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
            EthernetFramePayload::IPv4(ip) => match ip.payload() {
                IPv4PacketPayload::UDP(udp) => {
                    let src = SocketAddr::new(ip.src_ip(), udp.src_port());
                    self.udp_rx
                        .lock()
                        .dispatch(udp.dst_port(), src, udp.raw_payload().to_vec());
                }
                IPv4PacketPayload::TCP(tcp) => {
                    let src = SocketAddr::new(ip.src_ip(), tcp.src_port());
                    // Dispatch full TCP segment (header + payload) for state machine
                    let ip_hdr_len = ip.header_len();
                    let ip_total = ip.total_length() as usize;
                    let tcp_data = &packet[14 + ip_hdr_len..14 + ip_total]; // skip eth(14) + ip header
                    self.tcp_rx.lock().dispatch(tcp.dst_port(), src, tcp_data.to_vec());
                }
                IPv4PacketPayload::ICMP(_icmp) => {
                    // TODO: handle ICMP
                }
                IPv4PacketPayload::Raw(_) => {}
            },
            EthernetFramePayload::ARP(arp) => {
                self.handle_arp(&arp);
            }
            EthernetFramePayload::Raw(_) => {}
        }
    }
}
