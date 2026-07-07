pub mod inet;
pub mod netlink;
pub mod raw;
pub mod tcp;
pub mod udp;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use core::time::Duration;
use num_enum::TryFromPrimitive;

use crate::fs::file::FileFlags;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, FileEvent};
use crate::kernel::syscall::UserStruct;

pub use inet::InetSocket;
pub use netlink::{NetlinkProtocol, NetlinkSocket};

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
pub enum AddressFamily {
    Unix = 1,
    Inet = 2,
    Netlink = 16,
}

pub const SOCK_NONBLOCK: usize = 0x800;
pub const SOCK_CLOEXEC: usize = 0x80000;

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
pub enum ShutdownHow {
    Read = 0,
    Write = 1,
    ReadWrite = 2,
}

/// SHUT_* constants for shutdown()
pub const SHUT_RD: usize = ShutdownHow::Read as usize;
pub const SHUT_WR: usize = ShutdownHow::Write as usize;
pub const SHUT_RDWR: usize = ShutdownHow::ReadWrite as usize;

/// Kernel-internal socket address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
        if raw.sin_family != AddressFamily::Inet as u16 {
            return Err(Errno::EAFNOSUPPORT);
        }
        let ip = Ipv4Addr::from(u32::from_be(raw.sin_addr));
        let port = u16::from_be(raw.sin_port);
        Ok(Self { ip, port })
    }

    pub fn to_raw(&self) -> SockAddrIn {
        SockAddrIn {
            sin_family: AddressFamily::Inet as u16,
            sin_port: self.port.to_be(),
            sin_addr: u32::from(self.ip).to_be(),
            sin_zero: [0u8; 8],
        }
    }
}

/// Mirrors `struct sockaddr_in` from Linux UAPI (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, UserStruct)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

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

    fn recvfrom(
        &mut self,
        buf: &mut [u8],
        blocked: bool,
        timeout: Option<Duration>,
    ) -> SysResult<(usize, Option<SocketAddr>)>;

    fn shutdown(&mut self, _how: usize) -> SysResult<()> {
        Ok(())
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        None
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        None
    }

    fn poll_read(&mut self) -> bool;

    fn poll_write(&self) -> bool {
        true
    }

    fn wait_event(&mut self, event: FileEvent) -> Option<FileEvent> {
        let mut ready = FileEvent::empty();

        if event.contains(FileEvent::READ_READY) && self.poll_read() {
            ready |= FileEvent::READ_READY;
        }
        if event.contains(FileEvent::WRITE_READY) && self.poll_write() {
            ready |= FileEvent::WRITE_READY;
        }

        if ready.is_empty() { None } else { Some(ready) }
    }

    fn wait_read(&self, _waker: usize) -> bool {
        false
    }

    fn cancel_wait_read(&self) {}

    fn epoll_notifiers(&self) -> Vec<Arc<EpollNotifier>> {
        Vec::new()
    }

    fn set_flags(&mut self, _flags: &FileFlags) {}

    fn type_name(&self) -> &'static str;
}
