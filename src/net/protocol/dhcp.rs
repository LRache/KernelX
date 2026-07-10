use alloc::vec::Vec;

use crate::kernel::errno::SysResult;
use crate::net::protocol::{MacAddr, ProtocolBuilder};

/// DHCP message types
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum DhcpMessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

impl DhcpMessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Discover),
            2 => Some(Self::Offer),
            3 => Some(Self::Request),
            4 => Some(Self::Decline),
            5 => Some(Self::Ack),
            6 => Some(Self::Nak),
            7 => Some(Self::Release),
            8 => Some(Self::Inform),
            _ => None,
        }
    }
}

/// DHCP operation codes
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum DhcpOp {
    Request = 1,
    Reply = 2,
}

pub struct DHCPBuilder {
    op: u8,
    htype: u8,
    hlen: u8,
    hops: u8,
    xid: u32,
    secs: u16,
    flags: u16,
    ciaddr: [u8; 4],
    yiaddr: [u8; 4],
    siaddr: [u8; 4],
    giaddr: [u8; 4],
    chaddr: [u8; 16],
    sname: [u8; 64],
    file: [u8; 128],
    options: Vec<u8>,
}

/// DHCP header size (without options): 236 bytes
pub const DHCP_HEADER_SIZE: usize = 236;
/// DHCP magic cookie
const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

impl DHCPBuilder {
    pub fn new() -> Self {
        DHCPBuilder {
            op: DhcpOp::Request as u8,
            htype: 1, // Ethernet
            hlen: 6,
            hops: 0,
            xid: 0,
            secs: 0,
            flags: 0x8000, // Broadcast flag
            ciaddr: [0; 4],
            yiaddr: [0; 4],
            siaddr: [0; 4],
            giaddr: [0; 4],
            chaddr: [0; 16],
            sname: [0; 64],
            file: [0; 128],
            options: Vec::new(),
        }
    }

    pub fn op(mut self, op: DhcpOp) -> Self {
        self.op = op as u8;
        self
    }

    pub fn xid(mut self, xid: u32) -> Self {
        self.xid = xid;
        self
    }

    pub fn chaddr(mut self, mac: MacAddr) -> Self {
        self.chaddr[..6].copy_from_slice(mac.as_octets());
        self
    }

    pub fn flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    pub fn ciaddr(mut self, addr: [u8; 4]) -> Self {
        self.ciaddr = addr;
        self
    }

    pub fn siaddr(mut self, addr: [u8; 4]) -> Self {
        self.siaddr = addr;
        self
    }

    pub fn add_option(mut self, code: u8, data: &[u8]) -> Self {
        self.options.push(code);
        self.options.push(data.len() as u8);
        self.options.extend_from_slice(data);
        self
    }

    pub fn message_type(self, msg_type: DhcpMessageType) -> Self {
        self.add_option(53, &[msg_type as u8])
    }
}

impl ProtocolBuilder for DHCPBuilder {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        let total = DHCP_HEADER_SIZE + 4 + self.options.len() + 1; // header + cookie + options + end
        if data.len() < total {
            return Ok(0);
        }

        data[0] = self.op;
        data[1] = self.htype;
        data[2] = self.hlen;
        data[3] = self.hops;
        data[4..8].copy_from_slice(&self.xid.to_be_bytes());
        data[8..10].copy_from_slice(&self.secs.to_be_bytes());
        data[10..12].copy_from_slice(&self.flags.to_be_bytes());
        data[12..16].copy_from_slice(&self.ciaddr);
        data[16..20].copy_from_slice(&self.yiaddr);
        data[20..24].copy_from_slice(&self.siaddr);
        data[24..28].copy_from_slice(&self.giaddr);
        data[28..44].copy_from_slice(&self.chaddr);
        data[44..108].copy_from_slice(&self.sname);
        data[108..236].copy_from_slice(&self.file);

        // Magic cookie
        let mut offset = DHCP_HEADER_SIZE;
        data[offset..offset + 4].copy_from_slice(&DHCP_MAGIC_COOKIE);
        offset += 4;

        // Options
        data[offset..offset + self.options.len()].copy_from_slice(&self.options);
        offset += self.options.len();

        // End option
        data[offset] = 255;
        offset += 1;

        Ok(offset)
    }

    fn len(&self) -> usize {
        DHCP_HEADER_SIZE + 4 + self.options.len() + 1
    }
}

// ---- Parser ----

/// DHCP client/server port constants
pub const DHCP_SERVER_PORT: u16 = 67;
pub const DHCP_CLIENT_PORT: u16 = 68;

pub struct DHCPPacket<'a> {
    data: &'a [u8],
}

impl<'a> DHCPPacket<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < DHCP_HEADER_SIZE + 4 {
            return None;
        }
        // Verify magic cookie
        if data[DHCP_HEADER_SIZE..DHCP_HEADER_SIZE + 4] != DHCP_MAGIC_COOKIE {
            return None;
        }
        Some(Self { data })
    }

    pub fn op(&self) -> u8 {
        self.data[0]
    }

    pub fn xid(&self) -> u32 {
        u32::from_be_bytes([self.data[4], self.data[5], self.data[6], self.data[7]])
    }

    pub fn ciaddr(&self) -> [u8; 4] {
        [self.data[12], self.data[13], self.data[14], self.data[15]]
    }

    pub fn yiaddr(&self) -> [u8; 4] {
        [self.data[16], self.data[17], self.data[18], self.data[19]]
    }

    pub fn siaddr(&self) -> [u8; 4] {
        [self.data[20], self.data[21], self.data[22], self.data[23]]
    }

    pub fn chaddr(&self) -> &[u8] {
        &self.data[28..44]
    }

    /// Parse into a fully decoded DhcpReply (only valid for reply packets).
    pub fn as_reply(&self) -> Option<DhcpReply> {
        DhcpReply::parse(self.data)
    }
}

/// Parsed DHCP reply
pub struct DhcpReply {
    pub msg_type: DhcpMessageType,
    pub xid: u32,
    pub your_ip: [u8; 4],
    pub server_ip: [u8; 4],
    pub subnet_mask: Option<[u8; 4]>,
    pub router: Option<[u8; 4]>,
    pub dns: Option<[u8; 4]>,
    pub lease_time: Option<u32>,
}

impl DhcpReply {
    /// Parse a DHCP reply from raw UDP payload (DHCP data starting at op field).
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < DHCP_HEADER_SIZE + 4 {
            return None;
        }

        // op must be Reply (2)
        if data[0] != DhcpOp::Reply as u8 {
            return None;
        }

        let xid = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let your_ip = [data[16], data[17], data[18], data[19]];
        let server_ip = [data[20], data[21], data[22], data[23]];

        // Verify magic cookie
        let cookie_offset = DHCP_HEADER_SIZE;
        if data[cookie_offset..cookie_offset + 4] != DHCP_MAGIC_COOKIE {
            return None;
        }

        let mut msg_type = None;
        let mut subnet_mask = None;
        let mut router = None;
        let mut dns = None;
        let mut lease_time = None;

        // Parse options
        let mut i = cookie_offset + 4;
        while i < data.len() {
            let code = data[i];
            if code == 255 {
                break; // End
            }
            if code == 0 {
                i += 1; // Pad
                continue;
            }
            if i + 1 >= data.len() {
                break;
            }
            let len = data[i + 1] as usize;
            let opt_data = &data[i + 2..i + 2 + len.min(data.len() - i - 2)];

            match code {
                53 if opt_data.len() >= 1 => {
                    msg_type = DhcpMessageType::from_u8(opt_data[0]);
                }
                1 if opt_data.len() >= 4 => {
                    subnet_mask = Some([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]);
                }
                3 if opt_data.len() >= 4 => {
                    router = Some([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]);
                }
                6 if opt_data.len() >= 4 => {
                    dns = Some([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]);
                }
                51 if opt_data.len() >= 4 => {
                    lease_time = Some(u32::from_be_bytes([opt_data[0], opt_data[1], opt_data[2], opt_data[3]]));
                }
                _ => {}
            }
            i += 2 + len;
        }

        Some(DhcpReply {
            msg_type: msg_type?,
            xid,
            your_ip,
            server_ip,
            subnet_mask,
            router,
            dns,
            lease_time,
        })
    }
}
