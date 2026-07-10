use bitflags::bitflags;
use core::net::Ipv4Addr;

use crate::kernel::errno::SysResult;
use crate::net::protocol::ProtocolBuilder;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TcpFlags: u8 {
        const FIN = 0x01;
        const SYN = 0x02;
        const RST = 0x04;
        const PSH = 0x08;
        const ACK = 0x10;
        const URG = 0x20;
        const ECE = 0x40;
        const CWR = 0x80;
    }
}

pub struct TCPBuilder<'a> {
    src_port: u16,
    dst_port: u16,
    seq_num: u32,
    ack_num: u32,
    data_offset: u8,
    flags: TcpFlags,
    window_size: u16,
    checksum: u16,
    urgent_pointer: u16,
    payload: Option<&'a dyn ProtocolBuilder>,
    pseudo_src_ip: Option<Ipv4Addr>,
    pseudo_dst_ip: Option<Ipv4Addr>,
}

impl<'a> TCPBuilder<'a> {
    pub fn new(src_port: u16, dst_port: u16) -> Self {
        TCPBuilder {
            src_port,
            dst_port,
            seq_num: 0,
            ack_num: 0,
            data_offset: 5, // No options
            flags: TcpFlags::empty(),
            window_size: 65535,
            checksum: 0,
            urgent_pointer: 0,
            payload: None,
            pseudo_src_ip: None,
            pseudo_dst_ip: None,
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

    pub fn seq_num(mut self, num: u32) -> Self {
        self.seq_num = num;
        self
    }

    pub fn ack_num(mut self, num: u32) -> Self {
        self.ack_num = num;
        self
    }

    pub fn flags(mut self, flags: TcpFlags) -> Self {
        self.flags = flags;
        self
    }

    pub fn window_size(mut self, size: u16) -> Self {
        self.window_size = size;
        self
    }

    pub fn checksum(mut self, checksum: u16) -> Self {
        self.checksum = checksum;
        self
    }

    pub fn urgent_pointer(mut self, pointer: u16) -> Self {
        self.urgent_pointer = pointer;
        self
    }

    pub fn payload(mut self, payload: &'a dyn ProtocolBuilder) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn ipv4_pseudo_header(mut self, src_ip: Ipv4Addr, dst_ip: Ipv4Addr) -> Self {
        self.pseudo_src_ip = Some(src_ip);
        self.pseudo_dst_ip = Some(dst_ip);
        self
    }

    fn compute_ipv4_checksum(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, segment: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        let src = src_ip.octets();
        let dst = dst_ip.octets();

        // IPv4 pseudo-header
        sum += u16::from_be_bytes([src[0], src[1]]) as u32;
        sum += u16::from_be_bytes([src[2], src[3]]) as u32;
        sum += u16::from_be_bytes([dst[0], dst[1]]) as u32;
        sum += u16::from_be_bytes([dst[2], dst[3]]) as u32;
        sum += 6; // IPPROTO_TCP
        sum += segment.len() as u32;

        // TCP segment (checksum field should be zero when computing)
        let mut i = 0usize;
        while i + 1 < segment.len() {
            sum += u16::from_be_bytes([segment[i], segment[i + 1]]) as u32;
            i += 2;
        }
        if i < segment.len() {
            sum += (segment[i] as u32) << 8;
        }

        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        !(sum as u16)
    }
}

// ---- Parser ----

const TCP_HEADER_SIZE: usize = 20;

pub struct TCPPacket<'a> {
    data: &'a [u8],
}

impl<'a> TCPPacket<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < TCP_HEADER_SIZE {
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

    pub fn seq_num(&self) -> u32 {
        u32::from_be_bytes([self.data[4], self.data[5], self.data[6], self.data[7]])
    }

    pub fn ack_num(&self) -> u32 {
        u32::from_be_bytes([self.data[8], self.data[9], self.data[10], self.data[11]])
    }

    pub fn header_len(&self) -> usize {
        ((self.data[12] >> 4) as usize) * 4
    }

    pub fn flags(&self) -> TcpFlags {
        TcpFlags::from_bits_truncate(self.data[13])
    }

    pub fn window_size(&self) -> u16 {
        u16::from_be_bytes([self.data[14], self.data[15]])
    }

    pub fn payload(&self) -> &'a [u8] {
        &self.data[self.header_len()..]
    }
}

impl<'a> ProtocolBuilder for TCPBuilder<'a> {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        if data.len() < 20 {
            return Ok(0); // Not enough space for TCP header
        }

        // Build TCP header
        data[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        data[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        data[4..8].copy_from_slice(&self.seq_num.to_be_bytes());
        data[8..12].copy_from_slice(&self.ack_num.to_be_bytes());
        data[12] = (self.data_offset << 4) | 0; // No options
        data[13] = self.flags.bits();
        data[14..16].copy_from_slice(&self.window_size.to_be_bytes());
        data[16..18].copy_from_slice(&0u16.to_be_bytes());
        data[18..20].copy_from_slice(&self.urgent_pointer.to_be_bytes());

        let total_len = self.payload.as_ref().map_or(Ok(20), |p| {
            let payload_len = p.build(&mut data[20..])?;
            Ok(20 + payload_len)
        })?;

        let checksum = if let (Some(src_ip), Some(dst_ip)) = (self.pseudo_src_ip, self.pseudo_dst_ip) {
            Self::compute_ipv4_checksum(src_ip, dst_ip, &data[..total_len])
        } else {
            self.checksum
        };
        data[16..18].copy_from_slice(&checksum.to_be_bytes());

        Ok(total_len)
    }

    fn len(&self) -> usize {
        20 + self.payload.as_ref().map_or(0, |p| p.len())
    }
}
