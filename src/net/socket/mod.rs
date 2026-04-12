pub mod inet;
pub mod netlink;
pub mod options;
pub mod raw;
pub mod tcp;
pub mod udp;

use alloc::sync::Arc;
use core::net::Ipv4Addr;

use crate::fs::file::FileFlags;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{FileEvent, PollEventSet};
use crate::kernel::syscall::UserStruct;

pub use inet::InetSocket;
pub use netlink::NetlinkSocket;

pub const AF_UNIX: usize = 1;
pub const AF_INET: usize = 2;
pub const AF_NETLINK: usize = 16;

pub const SOCK_STREAM: usize = 1;
pub const SOCK_DGRAM: usize = 2;
pub const SOCK_RAW: usize = 3;

pub const SOCK_NONBLOCK: usize = 0x800;
pub const SOCK_CLOEXEC: usize = 0x80000;

/// SHUT_* constants for shutdown()
pub const SHUT_RD: usize = 0;
pub const SHUT_WR: usize = 1;
pub const SHUT_RDWR: usize = 2;

/// Kernel-internal socket address.
#[derive(Clone, Copy, Debug)]
pub struct SocketAddr {
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl SocketAddr {
    pub fn new(ip: Ipv4Addr, port: u16) -> Self {
        Self { ip, port }
    }

    pub fn any(port: u16) -> Self {
        Self {
            ip: Ipv4Addr::UNSPECIFIED,
            port,
        }
    }

    pub fn from_raw(raw: &SockAddrIn) -> SysResult<Self> {
        if raw.sin_family != AF_INET as u16 {
            return Err(Errno::EAFNOSUPPORT);
        }
        let ip = Ipv4Addr::from(u32::from_be(raw.sin_addr));
        let port = u16::from_be(raw.sin_port);
        Ok(Self { ip, port })
    }

    pub fn to_raw(&self) -> SockAddrIn {
        SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: self.port.to_be(),
            sin_addr: u32::from(self.ip).to_be(),
            sin_zero: [0u8; 8],
        }
    }
}

/// Mirrors `struct sockaddr_in` from Linux UAPI (16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

impl UserStruct for SockAddrIn {}

/// Trait for protocol-specific socket behavior (UDP, TCP, etc.).
pub trait SocketInner: Send + Sync {
    fn bind(&mut self, addr: SocketAddr) -> SysResult<()>;

    fn connect(&mut self, addr: SocketAddr, blocked: bool) -> SysResult<()>;

    fn listen(&mut self, _backlog: usize) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn accept(&mut self, _blocked: bool) -> SysResult<Arc<InetSocket>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn sendto(&mut self, buf: &[u8], dst: Option<SocketAddr>, blocked: bool) -> SysResult<usize>;

    fn recvfrom(&mut self, buf: &mut [u8], blocked: bool) -> SysResult<(usize, Option<SocketAddr>)>;

    fn shutdown(&mut self, _how: usize) -> SysResult<()> {
        Ok(())
    }

    fn poll_read(&self) -> bool;

    fn poll_write(&self) -> bool {
        true
    }

    fn wait_event(&self, event: PollEventSet) -> Option<FileEvent> {
        if event.contains(PollEventSet::POLLIN) && self.poll_read() {
            return Some(FileEvent::ReadReady);
        }
        if event.contains(PollEventSet::POLLOUT) && self.poll_write() {
            return Some(FileEvent::WriteReady);
        }
        None
    }

    fn set_flags(&mut self, _flags: &FileFlags) {}

    fn type_name(&self) -> &'static str;
}
