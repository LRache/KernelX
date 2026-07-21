use crate::kernel::errno::SysResult;

pub mod arp;
pub mod dhcp;
pub mod ethernet;
pub mod icmp;
pub mod ipv4;
pub mod tcp;
pub mod udp;

pub use arp::{ARPBuilder, ARPPacket, ArpOperation};
pub use ethernet::{EthernetBuilder, EthernetFrame, EthernetFramePayload, MacAddr};
pub use ipv4::{IPv4Builder, IPv4Packet, IPv4PacketPayload};
pub use tcp::{TCPBuilder, TCPPacket, TcpFlags};
pub use udp::UDPBuilder;

pub trait ProtocolBuilder {
    fn build(&self, data: &mut [u8]) -> SysResult<usize>;
    fn len(&self) -> usize;
}

/// Wraps a raw byte slice as a ProtocolBuilder payload.
pub struct RawPayload<'a>(pub &'a [u8]);

impl<'a> ProtocolBuilder for RawPayload<'a> {
    fn build(&self, data: &mut [u8]) -> SysResult<usize> {
        let n = self.0.len().min(data.len());
        data[..n].copy_from_slice(&self.0[..n]);
        Ok(n)
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}
