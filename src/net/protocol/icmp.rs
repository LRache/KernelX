use crate::kernel::errno::SysResult;
use crate::net::protocol::ProtocolBuilder;

/// Common ICMP types
pub const ICMP_ECHO_REPLY: u8 = 0;
pub const ICMP_ECHO_REQUEST: u8 = 8;

pub struct ICMPBuilder {
    icmp_type: u8,
    code: u8,
    checksum: u16,
    identifier: u16,
    sequence: u16,
}

/// ICMP header size: 8 bytes
const ICMP_HEADER_SIZE: usize = 8;

impl ICMPBuilder {
    pub fn new(icmp_type: u8, code: u8) -> Self {
        ICMPBuilder {
            icmp_type,
            code,
            checksum: 0,
            identifier: 0,
            sequence: 0,
        }
    }

    pub fn identifier(mut self, id: u16) -> Self {
        self.identifier = id;
        self
    }

    pub fn sequence(mut self, seq: u16) -> Self {
        self.sequence = seq;
        self
    }

    pub fn checksum(mut self, checksum: u16) -> Self {
        self.checksum = checksum;
        self
    }
}

impl ProtocolBuilder for ICMPBuilder {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        if data.len() < ICMP_HEADER_SIZE {
            return Ok(0);
        }

        data[0] = self.icmp_type;
        data[1] = self.code;
        data[2..4].copy_from_slice(&self.checksum.to_be_bytes());
        data[4..6].copy_from_slice(&self.identifier.to_be_bytes());
        data[6..8].copy_from_slice(&self.sequence.to_be_bytes());

        Ok(ICMP_HEADER_SIZE)
    }

    fn len(&self) -> usize {
        ICMP_HEADER_SIZE
    }
}

// ---- Parser ----

pub struct ICMPPacket<'a> {
    data: &'a [u8],
}

impl<'a> ICMPPacket<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < ICMP_HEADER_SIZE {
            return None;
        }
        Some(Self { data })
    }

    pub fn icmp_type(&self) -> u8 {
        self.data[0]
    }

    pub fn code(&self) -> u8 {
        self.data[1]
    }

    pub fn identifier(&self) -> u16 {
        u16::from_be_bytes([self.data[4], self.data[5]])
    }

    pub fn sequence(&self) -> u16 {
        u16::from_be_bytes([self.data[6], self.data[7]])
    }

    pub fn payload(&self) -> &'a [u8] {
        &self.data[ICMP_HEADER_SIZE..]
    }
}
