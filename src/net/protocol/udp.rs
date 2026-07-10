use crate::kernel::errno::SysResult;
use crate::net::protocol::ProtocolBuilder;

pub struct UDPBuilder<'a> {
    src_port: u16,
    dst_port: u16,
    checksum: u16,
    payload: Option<&'a dyn ProtocolBuilder>,
}

impl<'a> UDPBuilder<'a> {
    pub fn new() -> Self {
        UDPBuilder {
            src_port: 0,
            dst_port: 0,
            checksum: 0,
            payload: None,
        }
    }

    pub fn src_port(mut self, port: u16) -> Self {
        self.src_port = port;
        self
    }

    pub fn dst_port(mut self, port: u16) -> Self {
        self.dst_port = port;
        self
    }

    pub fn checksum(mut self, checksum: u16) -> Self {
        self.checksum = checksum;
        self
    }

    pub fn payload(mut self, payload: &'a dyn ProtocolBuilder) -> Self {
        self.payload = Some(payload);
        self
    }
}

// ---- Parser ----

const UDP_HEADER_SIZE: usize = 8;

pub struct UDPPacket<'a> {
    data: &'a [u8],
}

impl<'a> UDPPacket<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < UDP_HEADER_SIZE {
            return None;
        }
        Some(Self { data })
    }

    pub fn src_port(&self) -> u16 {
        u16::from_be_bytes([self.data[0], self.data[1]])
    }

    pub fn dst_port(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }

    pub fn length(&self) -> u16 {
        u16::from_be_bytes([self.data[4], self.data[5]])
    }

    pub fn payload(&self) -> UDPPacketPayload<'a> {
        let data = &self.data[UDP_HEADER_SIZE..];
        let src = self.src_port();
        let dst = self.dst_port();
        // DHCP: server port 67, client port 68
        if (src == 67 && dst == 68) || (src == 68 && dst == 67) {
            if let Some(dhcp) = super::dhcp::DHCPPacket::parse(data) {
                return UDPPacketPayload::DHCP(dhcp);
            }
        }
        UDPPacketPayload::Raw(data)
    }

    pub fn raw_payload(&self) -> &'a [u8] {
        &self.data[UDP_HEADER_SIZE..]
    }
}

pub enum UDPPacketPayload<'a> {
    DHCP(super::dhcp::DHCPPacket<'a>),
    Raw(&'a [u8]),
}

impl<'a> ProtocolBuilder for UDPBuilder<'a> {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        if data.len() < 8 {
            return Ok(0); // Not enough space for UDP header
        }

        let length = self.len() as u16;

        // Build UDP header
        data[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        data[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        data[4..6].copy_from_slice(&length.to_be_bytes());
        data[6..8].copy_from_slice(&self.checksum.to_be_bytes());

        self.payload.as_ref().map_or(Ok(8), |p| {
            let payload_len = p.build(&mut data[8..])?;
            Ok(8 + payload_len)
        })
    }

    fn len(&self) -> usize {
        8 + self.payload.as_ref().map_or(0, |p| p.len())
    }
}
