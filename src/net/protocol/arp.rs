use core::net::Ipv4Addr;

use crate::kernel::errno::SysResult;
use crate::net::protocol::{MacAddr, ProtocolBuilder};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArpOperation {
    Request,
    Reply,
    Other(u16),
}

impl From<u16> for ArpOperation {
    fn from(value: u16) -> Self {
        match value {
            1 => ArpOperation::Request,
            2 => ArpOperation::Reply,
            other => ArpOperation::Other(other),
        }
    }
}

impl From<ArpOperation> for u16 {
    fn from(value: ArpOperation) -> Self {
        match value {
            ArpOperation::Request => 1,
            ArpOperation::Reply => 2,
            ArpOperation::Other(code) => code,
        }
    }
}

pub struct ARPBuilder {
    hlen: u8,
    plen: u8,
    operation: ArpOperation,
    sender_mac: MacAddr,
    sender_ip: Ipv4Addr,
    target_mac: MacAddr,
    target_ip: Ipv4Addr,
}

/// ARP packet size (for Ethernet/IPv4): 28 bytes
const ARP_SIZE: usize = 28;

impl ARPBuilder {
    pub fn new(
        operation: ArpOperation,
        sender_mac: MacAddr,
        sender_ip: Ipv4Addr,
        target_mac: MacAddr,
        target_ip: Ipv4Addr,
    ) -> Self {
        ARPBuilder {
            hlen: 6,
            plen: 4,
            operation,
            sender_mac,
            sender_ip,
            target_mac,
            target_ip,
        }
    }

    pub fn operation(mut self, op: ArpOperation) -> Self {
        self.operation = op;
        self
    }

    pub fn sender_mac(mut self, mac: MacAddr) -> Self {
        self.sender_mac = mac;
        self
    }

    pub fn sender_ip(mut self, ip: Ipv4Addr) -> Self {
        self.sender_ip = ip;
        self
    }

    pub fn target_mac(mut self, mac: MacAddr) -> Self {
        self.target_mac = mac;
        self
    }

    pub fn target_ip(mut self, ip: Ipv4Addr) -> Self {
        self.target_ip = ip;
        self
    }
}

impl ProtocolBuilder for ARPBuilder {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        if data.len() < ARP_SIZE {
            return Ok(0);
        }

        data[0..2].copy_from_slice(&1u16.to_be_bytes()); // Ethernet
        data[2..4].copy_from_slice(&0x0800u16.to_be_bytes()); // IPv4
        data[4] = self.hlen;
        data[5] = self.plen;
        data[6..8].copy_from_slice(&u16::from(self.operation).to_be_bytes());
        data[8..14].copy_from_slice(self.sender_mac.as_octets());
        data[14..18].copy_from_slice(&self.sender_ip.octets());
        data[18..24].copy_from_slice(self.target_mac.as_octets());
        data[24..28].copy_from_slice(&self.target_ip.octets());

        Ok(ARP_SIZE)
    }

    fn len(&self) -> usize {
        ARP_SIZE
    }
}

// ---- Parser ----

pub struct ARPPacket<'a> {
    data: &'a [u8],
}

impl<'a> ARPPacket<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < ARP_SIZE {
            return None;
        }
        Some(Self { data })
    }

    pub fn operation(&self) -> ArpOperation {
        u16::from_be_bytes([self.data[6], self.data[7]]).into()
    }

    pub fn sender_mac(&self) -> MacAddr {
        MacAddr::from_octets(self.data[8..14].try_into().unwrap())
    }

    pub fn sender_ip(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.data[14], self.data[15], self.data[16], self.data[17])
    }

    pub fn target_mac(&self) -> MacAddr {
        MacAddr::from_octets(self.data[18..24].try_into().unwrap())
    }

    pub fn target_ip(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.data[24], self.data[25], self.data[26], self.data[27])
    }
}
