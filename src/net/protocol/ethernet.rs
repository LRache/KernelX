use crate::kernel::errno::SysResult;
use crate::net::protocol::ProtocolBuilder;

use super::{ARPBuilder, ARPPacket, IPv4Builder, IPv4Packet};

#[derive(Clone, Copy, Debug)]
pub struct MacAddr([u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xff; 6]);
    pub const UNSPECIFIED: MacAddr = MacAddr([0; 6]);

    pub fn from_octets(octets: [u8; 6]) -> Self {
        MacAddr(octets)
    }

    pub fn as_octets(&self) -> &[u8; 6] {
        &self.0
    }
}

/// Common EtherType values
const ETHERTYPE_IPV4: u16 = 0x0800;
const ETHERTYPE_ARP: u16 = 0x0806;

pub enum EthernetPayload<'a> {
    IPv4(IPv4Builder<'a>),
    ARP(ARPBuilder),
}

impl<'a> EthernetPayload<'a> {
    fn ethertype(&self) -> u16 {
        match self {
            EthernetPayload::IPv4(_) => ETHERTYPE_IPV4,
            EthernetPayload::ARP(_) => ETHERTYPE_ARP,
        }
    }
}

impl<'a> ProtocolBuilder for EthernetPayload<'a> {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        match self {
            EthernetPayload::IPv4(b) => b.build(data),
            EthernetPayload::ARP(b) => b.build(data),
        }
    }

    fn len(&self) -> usize {
        match self {
            EthernetPayload::IPv4(b) => b.len(),
            EthernetPayload::ARP(b) => b.len(),
        }
    }
}

pub struct EthernetBuilder<'a> {
    dst_mac: MacAddr,
    src_mac: MacAddr,
    payload: Option<EthernetPayload<'a>>,
}

/// Ethernet header size: 14 bytes
const ETHERNET_HEADER_SIZE: usize = 14;

impl<'a> EthernetBuilder<'a> {
    pub fn new() -> Self {
        EthernetBuilder {
            dst_mac: MacAddr::UNSPECIFIED,
            src_mac: MacAddr::UNSPECIFIED,
            payload: None,
        }
    }

    pub fn dst_mac(mut self, mac: MacAddr) -> Self {
        self.dst_mac = mac;
        self
    }

    pub fn src_mac(mut self, mac: MacAddr) -> Self {
        self.src_mac = mac;
        self
    }

    pub fn ipv4(mut self, ipv4: IPv4Builder<'a>) -> Self {
        self.payload = Some(EthernetPayload::IPv4(ipv4));
        self
    }

    pub fn arp(mut self, arp: ARPBuilder) -> Self {
        self.payload = Some(EthernetPayload::ARP(arp));
        self
    }
}

impl<'a> ProtocolBuilder for EthernetBuilder<'a> {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        if data.len() < ETHERNET_HEADER_SIZE {
            return Ok(0);
        }

        let ethertype = self.payload.as_ref().map_or(0u16, |p| p.ethertype());

        data[0..6].copy_from_slice(self.dst_mac.as_octets());
        data[6..12].copy_from_slice(self.src_mac.as_octets());
        data[12..14].copy_from_slice(&ethertype.to_be_bytes());

        if let Some(p) = &self.payload {
            let written = p.build(&mut data[ETHERNET_HEADER_SIZE..])?;
            Ok(ETHERNET_HEADER_SIZE + written)
        } else {
            Ok(ETHERNET_HEADER_SIZE)
        }
    }

    fn len(&self) -> usize {
        ETHERNET_HEADER_SIZE + self.payload.as_ref().map_or(0, |p| p.len())
    }
}

pub const ETHERTYPE_IPV4_VAL: u16 = ETHERTYPE_IPV4;
pub const ETHERTYPE_ARP_VAL: u16 = ETHERTYPE_ARP;

pub struct EthernetFrame<'a> {
    data: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < ETHERNET_HEADER_SIZE {
            return None;
        }
        Some(Self { data })
    }

    pub fn dst_mac(&self) -> MacAddr {
        MacAddr::from_octets(self.data[0..6].try_into().unwrap())
    }

    pub fn src_mac(&self) -> MacAddr {
        MacAddr::from_octets(self.data[6..12].try_into().unwrap())
    }

    pub fn ethertype(&self) -> u16 {
        u16::from_be_bytes([self.data[12], self.data[13]])
    }

    pub fn payload(&self) -> EthernetFramePayload<'a> {
        let data = &self.data[ETHERNET_HEADER_SIZE..];
        match self.ethertype() {
            ETHERTYPE_IPV4 => {
                if let Some(ip) = IPv4Packet::parse(data) {
                    return EthernetFramePayload::IPv4(ip);
                }
            }
            ETHERTYPE_ARP => {
                if let Some(arp) = ARPPacket::parse(data) {
                    return EthernetFramePayload::ARP(arp);
                }
            }
            _ => {}
        }
        EthernetFramePayload::Raw(data)
    }
}

pub enum EthernetFramePayload<'a> {
    IPv4(IPv4Packet<'a>),
    ARP(ARPPacket<'a>),
    Raw(&'a [u8]),
}
