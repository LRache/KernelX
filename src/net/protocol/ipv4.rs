use core::net::Ipv4Addr;

use crate::kernel::errno::SysResult;
use crate::net::protocol::{ProtocolBuilder, RawPayload};

use super::icmp::ICMPBuilder;
use super::tcp::TCPBuilder;
use super::udp::UDPBuilder;

/// Common IPv4 protocol numbers
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

enum IPv4Payload<'a> {
    TCP(TCPBuilder<'a>),
    UDP(UDPBuilder<'a>),
    ICMP(ICMPBuilder),
    Raw { protocol: u8, payload: RawPayload<'a> },
}

impl<'a> IPv4Payload<'a> {
    fn protocol_number(&self) -> u8 {
        match self {
            IPv4Payload::TCP(_) => PROTO_TCP,
            IPv4Payload::UDP(_) => PROTO_UDP,
            IPv4Payload::ICMP(_) => PROTO_ICMP,
            IPv4Payload::Raw { protocol, .. } => *protocol,
        }
    }
}

impl<'a> ProtocolBuilder for IPv4Payload<'a> {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        match self {
            IPv4Payload::TCP(b) => b.build(data),
            IPv4Payload::UDP(b) => b.build(data),
            IPv4Payload::ICMP(b) => b.build(data),
            IPv4Payload::Raw { payload, .. } => payload.build(data),
        }
    }

    fn len(&self) -> usize {
        match self {
            IPv4Payload::TCP(b) => b.len(),
            IPv4Payload::UDP(b) => b.len(),
            IPv4Payload::ICMP(b) => b.len(),
            IPv4Payload::Raw { payload, .. } => payload.len(),
        }
    }
}

pub struct IPv4Builder<'a> {
    version_ihl: u8,
    dscp_ecn: u8,
    identification: u16,
    flags_fragment: u16,
    ttl: u8,
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    payload: Option<IPv4Payload<'a>>,
}

/// IPv4 header size (no options): 20 bytes
const IPV4_HEADER_SIZE: usize = 20;

impl<'a> IPv4Builder<'a> {
    pub fn new() -> Self {
        IPv4Builder {
            version_ihl: 0x45, // IPv4, IHL=5 (20 bytes)
            dscp_ecn: 0,
            identification: 0,
            flags_fragment: 0x4000, // Don't Fragment
            ttl: 64,
            src_ip: Ipv4Addr::UNSPECIFIED,
            dst_ip: Ipv4Addr::UNSPECIFIED,
            payload: None,
        }
    }

    pub fn src_ip(mut self, ip: Ipv4Addr) -> Self {
        self.src_ip = ip;
        self
    }

    pub fn dst_ip(mut self, ip: Ipv4Addr) -> Self {
        self.dst_ip = ip;
        self
    }

    pub fn ttl(mut self, ttl: u8) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn identification(mut self, id: u16) -> Self {
        self.identification = id;
        self
    }

    pub fn dscp_ecn(mut self, val: u8) -> Self {
        self.dscp_ecn = val;
        self
    }

    pub fn flags_fragment(mut self, val: u16) -> Self {
        self.flags_fragment = val;
        self
    }

    pub fn tcp(mut self, tcp: TCPBuilder<'a>) -> Self {
        self.payload = Some(IPv4Payload::TCP(tcp));
        self
    }

    pub fn udp(mut self, udp: UDPBuilder<'a>) -> Self {
        self.payload = Some(IPv4Payload::UDP(udp));
        self
    }

    pub fn icmp(mut self, icmp: ICMPBuilder) -> Self {
        self.payload = Some(IPv4Payload::ICMP(icmp));
        self
    }

    pub fn raw(mut self, protocol: u8, payload: &'a [u8]) -> Self {
        self.payload = Some(IPv4Payload::Raw {
            protocol,
            payload: RawPayload(payload),
        });
        self
    }

    /// Compute IPv4 header checksum over the 20-byte header in `data`.
    fn compute_checksum(header: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        for i in (0..20).step_by(2) {
            sum += u16::from_be_bytes([header[i], header[i + 1]]) as u32;
        }
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        !(sum as u16)
    }
}

// ---- Parser ----

pub struct IPv4Packet<'a> {
    data: &'a [u8],
}

impl<'a> IPv4Packet<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < IPV4_HEADER_SIZE {
            return None;
        }
        let ihl = ((data[0] & 0x0f) as usize) * 4;
        if data.len() < ihl {
            return None;
        }
        Some(Self { data })
    }

    pub fn header_len(&self) -> usize {
        ((self.data[0] & 0x0f) as usize) * 4
    }

    pub fn total_length(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }

    pub fn protocol(&self) -> u8 {
        self.data[9]
    }

    pub fn src_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from([self.data[12], self.data[13], self.data[14], self.data[15]])
    }

    pub fn dst_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from([self.data[16], self.data[17], self.data[18], self.data[19]])
    }

    pub fn payload(&self) -> IPv4PacketPayload<'a> {
        let data = &self.data[self.header_len()..];
        match self.protocol() {
            PROTO_TCP => {
                if let Some(tcp) = super::tcp::TCPPacket::parse(data) {
                    return IPv4PacketPayload::TCP(tcp);
                }
            }
            PROTO_UDP => {
                if let Some(udp) = super::udp::UDPPacket::parse(data) {
                    return IPv4PacketPayload::UDP(udp);
                }
            }
            PROTO_ICMP => {
                if let Some(icmp) = super::icmp::ICMPPacket::parse(data) {
                    return IPv4PacketPayload::ICMP(icmp);
                }
            }
            _ => {}
        }
        IPv4PacketPayload::Raw(data)
    }
}

pub enum IPv4PacketPayload<'a> {
    TCP(super::tcp::TCPPacket<'a>),
    UDP(super::udp::UDPPacket<'a>),
    ICMP(super::icmp::ICMPPacket<'a>),
    Raw(&'a [u8]),
}

impl<'a> ProtocolBuilder for IPv4Builder<'a> {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        if data.len() < IPV4_HEADER_SIZE {
            return Ok(0);
        }

        let payload_len = self.payload.as_ref().map_or(0, |p| p.len());
        let total_length = (IPV4_HEADER_SIZE + payload_len) as u16;
        let protocol = self.payload.as_ref().map_or(0, |p| p.protocol_number());

        data[0] = self.version_ihl;
        data[1] = self.dscp_ecn;
        data[2..4].copy_from_slice(&total_length.to_be_bytes());
        data[4..6].copy_from_slice(&self.identification.to_be_bytes());
        data[6..8].copy_from_slice(&self.flags_fragment.to_be_bytes());
        data[8] = self.ttl;
        data[9] = protocol;
        data[10..12].copy_from_slice(&0u16.to_be_bytes()); // checksum placeholder
        data[12..16].copy_from_slice(&self.src_ip.octets());
        data[16..20].copy_from_slice(&self.dst_ip.octets());

        // Compute and fill checksum
        let cksum = Self::compute_checksum(&data[..20]);
        data[10..12].copy_from_slice(&cksum.to_be_bytes());

        // Build payload
        if let Some(p) = &self.payload {
            let written = p.build(&mut data[IPV4_HEADER_SIZE..])?;
            Ok(IPV4_HEADER_SIZE + written)
        } else {
            Ok(IPV4_HEADER_SIZE)
        }
    }

    fn len(&self) -> usize {
        IPV4_HEADER_SIZE + self.payload.as_ref().map_or(0, |p| p.len())
    }
}
