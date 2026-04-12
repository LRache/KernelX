use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::fs::file::{FileFlags, FileOps, SeekWhence};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{FileEvent, PollEventSet};
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::raw::RawInner;
use super::tcp::TcpInner;
use super::udp::UdpInner;
use super::{SocketAddr, SocketInner};

pub struct InetSocket {
    inner: SpinLock<Box<dyn SocketInner>>,
    blocked: SpinLock<bool>,
}

impl InetSocket {
    pub fn new_udp(blocked: bool) -> Self {
        Self {
            inner: SpinLock::new(Box::new(UdpInner::new()), "InetSocket::inner"),
            blocked: SpinLock::new(blocked, "InetSocket::blocked"),
        }
    }

    pub fn new_tcp(blocked: bool) -> Self {
        Self {
            inner: SpinLock::new(Box::new(TcpInner::new()), "InetSocket::inner"),
            blocked: SpinLock::new(blocked, "InetSocket::blocked"),
        }
    }

    pub fn new_raw(protocol: u8, blocked: bool) -> Self {
        Self {
            inner: SpinLock::new(Box::new(RawInner::new(protocol)), "InetSocket::inner"),
            blocked: SpinLock::new(blocked, "InetSocket::blocked"),
        }
    }

    /// Create an InetSocket from a pre-built SocketInner (used by accept).
    pub fn from_inner(inner: Box<dyn SocketInner>, blocked: bool) -> Self {
        Self {
            inner: SpinLock::new(inner, "InetSocket::inner"),
            blocked: SpinLock::new(blocked, "InetSocket::blocked"),
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

    pub fn recvfrom(&self, buf: &mut [u8]) -> SysResult<(usize, Option<SocketAddr>)> {
        let blocked = *self.blocked.lock();
        self.inner.lock().recvfrom(buf, blocked)
    }

    pub fn shutdown(&self, how: usize) -> SysResult<()> {
        self.inner.lock().shutdown(how)
    }

    pub fn setsockopt(&self, _level: usize, _optname: usize, _optval: usize, _optlen: usize) -> SysResult<()> {
        Ok(())
    }

    pub fn getsockopt(&self, _level: usize, _optname: usize, _optval: usize, _optlen: usize) -> SysResult<()> {
        Ok(())
    }
}

impl FileOps for InetSocket {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        let (n, _) = self.inner.lock().recvfrom(buf, blocked)?;
        Ok(n)
    }

    fn pread(&self, _: &mut [u8], _: usize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let blocked = *self.blocked.lock();
        self.inner.lock().sendto(buf, None, blocked)
    }

    fn pwrite(&self, _: &[u8], _: usize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: true,
            writable: true,
            blocked: *self.blocked.lock(),
            append: false,
        }
    }

    fn seek(&self, _: isize, _: SeekWhence) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_mode = Mode::S_IFSOCK.bits() as u32 | 0o666;
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn wait_event(&self, _waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        Ok(self.inner.lock().wait_event(event))
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.blocked.lock() = flags.blocked;
        self.inner.lock().set_flags(&flags);
    }

    fn type_name(&self) -> &'static str {
        self.inner.lock().type_name()
    }
}
