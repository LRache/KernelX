use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use core::time::Duration;

use num_enum::TryFromPrimitive;

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, Inode, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, FileEvent};
use crate::kernel::uapi::FileStat;
use crate::klib::{SleepLock, SpinLock};
use crate::net::protocol::ipv4::IpProtocol;

use super::raw::RawInner;
use super::tcp::TcpInner;
use super::udp::UdpInner;
use super::{SocketAddr, SocketInner};

const DEFAULT_SOCKET_BUFFER_SIZE: usize = 64 * 1024;
const TCP_MAX_SEGMENT_SIZE: usize = 1460;

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum SocketOption {
    ReuseAddr = 2,
    Type = 3,
    Error = 4,
    SendBuffer = 7,
    RecvBuffer = 8,
    RecvTimeout = 20,
    RecvTimeoutNew = 66,
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
enum TcpOption {
    MaxSegment = 2,
}

#[derive(Clone, Copy)]
struct SocketOptions {
    send_buffer_size: usize,
    recv_buffer_size: usize,
    reuse_addr: bool,
    recv_timeout: Option<Duration>,
}

impl SocketOptions {
    fn new() -> Self {
        Self {
            send_buffer_size: DEFAULT_SOCKET_BUFFER_SIZE,
            recv_buffer_size: DEFAULT_SOCKET_BUFFER_SIZE,
            reuse_addr: false,
            recv_timeout: None,
        }
    }
}

pub struct InetSocket {
    inner: SleepLock<Box<dyn SocketInner>>,
    blocked: SpinLock<bool>,
    options: SpinLock<SocketOptions>,
}

impl InetSocket {
    pub fn new_udp(blocked: bool) -> Self {
        Self {
            inner: SleepLock::new(Box::new(UdpInner::new()), "InetSocket::inner"),
            blocked: SpinLock::new(blocked, "InetSocket::blocked"),
            options: SpinLock::new(SocketOptions::new(), "InetSocket::options"),
        }
    }

    pub fn new_tcp(blocked: bool) -> Self {
        Self {
            inner: SleepLock::new(Box::new(TcpInner::new()), "InetSocket::inner"),
            blocked: SpinLock::new(blocked, "InetSocket::blocked"),
            options: SpinLock::new(SocketOptions::new(), "InetSocket::options"),
        }
    }

    pub fn new_raw(protocol: u8, blocked: bool) -> Self {
        Self {
            inner: SleepLock::new(Box::new(RawInner::new(protocol)), "InetSocket::inner"),
            blocked: SpinLock::new(blocked, "InetSocket::blocked"),
            options: SpinLock::new(SocketOptions::new(), "InetSocket::options"),
        }
    }

    /// Create an InetSocket from a pre-built SocketInner (used by accept).
    pub fn from_inner(inner: Box<dyn SocketInner>, blocked: bool) -> Self {
        Self {
            inner: SleepLock::new(inner, "InetSocket::inner"),
            blocked: SpinLock::new(blocked, "InetSocket::blocked"),
            options: SpinLock::new(SocketOptions::new(), "InetSocket::options"),
        }
    }

    // --- Socket operations callable from syscall layer ---

    pub fn bind(&self, addr: SocketAddr) -> SysResult<()> {
        self.inner.lock().bind(addr)
    }

    pub fn connect(&self, addr: SocketAddr) -> SysResult<()> {
        let blocked = *self.blocked.lock();
        self.inner.lock().connect(addr, blocked)
    }

    pub fn listen(&self, backlog: usize) -> SysResult<()> {
        self.inner.lock().listen(backlog)
    }

    pub fn accept(&self) -> SysResult<Arc<InetSocket>> {
        let blocked = *self.blocked.lock();
        self.inner.lock().accept(blocked)
    }

    pub fn sendto(&self, buf: &[u8], dst: Option<SocketAddr>) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        self.inner.lock().sendto(buf, dst, blocked)
    }

    pub fn sendto_with_blocking(&self, buf: &[u8], dst: Option<SocketAddr>, allow_block: bool) -> SysResult<usize> {
        let blocked = *self.blocked.lock() && allow_block;
        self.inner.lock().sendto(buf, dst, blocked)
    }

    pub fn recvfrom(&self, buf: &mut [u8]) -> SysResult<(usize, Option<SocketAddr>)> {
        let blocked = *self.blocked.lock();
        let timeout = self.options.lock().recv_timeout;
        self.inner.lock().recvfrom(buf, blocked, timeout)
    }

    pub fn recvfrom_with_blocking(&self, buf: &mut [u8], allow_block: bool) -> SysResult<(usize, Option<SocketAddr>)> {
        let blocked = *self.blocked.lock() && allow_block;
        let timeout = self.options.lock().recv_timeout;
        self.inner.lock().recvfrom(buf, blocked, timeout)
    }

    pub fn shutdown(&self, how: usize) -> SysResult<()> {
        self.inner.lock().shutdown(how)
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.inner.lock().local_addr()
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.inner.lock().peer_addr()
    }

    pub fn setsockopt(&self, level: usize, optname: usize, value: usize) -> SysResult<()> {
        if level != 1 {
            return Ok(());
        }

        let Ok(option) = SocketOption::try_from(optname) else {
            return Ok(());
        };
        let mut options = self.options.lock();
        match option {
            SocketOption::ReuseAddr => options.reuse_addr = value != 0,
            SocketOption::SendBuffer => options.send_buffer_size = value.max(1),
            SocketOption::RecvBuffer => options.recv_buffer_size = value.max(1),
            SocketOption::RecvTimeout | SocketOption::RecvTimeoutNew => {}
            SocketOption::Type | SocketOption::Error => {}
        }
        Ok(())
    }

    pub fn set_recv_timeout(&self, timeout: Option<Duration>) {
        self.options.lock().recv_timeout = timeout;
    }

    pub fn getsockopt(&self, level: usize, optname: usize) -> SysResult<usize> {
        if level == 1 {
            let options = self.options.lock();
            return Ok(match SocketOption::try_from(optname) {
                Ok(SocketOption::ReuseAddr) => usize::from(options.reuse_addr),
                Ok(SocketOption::SendBuffer) => options.send_buffer_size,
                Ok(SocketOption::RecvBuffer) => options.recv_buffer_size,
                Ok(SocketOption::RecvTimeout | SocketOption::RecvTimeoutNew) => 0,
                Ok(SocketOption::Type) => self.socket_type(),
                Ok(SocketOption::Error) => 0,
                Err(_) => 0,
            });
        }

        if level == IpProtocol::Tcp as usize && matches!(TcpOption::try_from(optname), Ok(TcpOption::MaxSegment)) {
            return Ok(TCP_MAX_SEGMENT_SIZE);
        }

        Ok(0)
    }

    fn socket_type(&self) -> usize {
        match self.inner.lock().type_name() {
            "inet-tcp" => 1,
            "inet-udp" => 2,
            "inet-raw" => 3,
            _ => 0,
        }
    }
}

impl FileOps for InetSocket {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let (n, _) = self.recvfrom(buf)?;
        Ok(n)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.sendto(buf, None)
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: true,
            writable: true,
            blocked: *self.blocked.lock(),
            append: false,
            direct: false,
        }
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_mode = Mode::S_IFSOCK.bits() as u32 | 0o666;
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Err(Errno::EINVAL)
    }

    fn get_inode(&self) -> Option<&Arc<Inode>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        Ok(self.inner.lock().wait_event(event))
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let mut inner = self.inner.lock();
        if let Some(ready) = inner.wait_event(event) {
            return Ok(Some(ready));
        }
        if event.contains(FileEvent::READ_READY) && inner.wait_read(waker) {
            return Ok(Some(FileEvent::READ_READY));
        }
        Ok(None)
    }

    fn wait_event_cancel(&self) {
        self.inner.lock().cancel_wait_read();
    }

    fn epoll_notifiers(&self) -> Option<Vec<Arc<EpollNotifier>>> {
        let notifiers = self.inner.lock().epoll_notifiers();
        if notifiers.is_empty() { None } else { Some(notifiers) }
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.blocked.lock() = flags.blocked;
        self.inner.lock().set_flags(&flags);
    }

    fn type_name(&self) -> &'static str {
        self.inner.lock().type_name()
    }
}
