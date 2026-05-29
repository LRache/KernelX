use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, FileEvent};
use crate::kernel::mm::AddrSpace;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::{FileStat, Statfs};
use crate::klib::SpinLock;

use super::PipeInner;

struct Meta {
    inode: Arc<dyn InodeOps>,
    dentry: Arc<Dentry>,
}

pub struct Pipe {
    inner: Arc<PipeInner>,
    meta: Option<Meta>,
    readable: bool,
    writable: bool,
    blocked: SpinLock<bool>,
}

impl Pipe {
    pub fn new(inner: Arc<PipeInner>, writable: bool, blocked: bool) -> Self {
        Self::new_inner(inner, !writable, writable, None, blocked)
    }

    fn new_inner(inner: Arc<PipeInner>, readable: bool, writable: bool, meta: Option<Meta>, blocked: bool) -> Self {
        if readable {
            inner.increment_reader_count();
        }
        if writable {
            inner.increment_writer_count();
        }
        Self {
            inner,
            meta,
            readable,
            writable,
            blocked: SpinLock::new(blocked, "Pipe::blocked"),
        }
    }

    pub fn create(capacity: usize, blocked: bool) -> (Self, Self) {
        let inner = Arc::new(PipeInner::new(capacity));
        let read_end = Pipe::new(inner.clone(), false, blocked);
        let write_end = Pipe::new(inner, true, blocked);
        (read_end, write_end)
    }

    pub fn open_fifo(
        inner: Arc<PipeInner>,
        inode: Arc<dyn InodeOps>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> SysResult<Arc<dyn FileOps>> {
        let dentry = dentry.ok_or(Errno::EINVAL)?;
        if flags.writable && !flags.readable && !flags.blocked && !inner.has_readers() {
            return Err(Errno::ENXIO);
        }

        let pipe = Arc::new(Self::new_inner(
            inner,
            flags.readable,
            flags.writable,
            Some(Meta { inode, dentry }),
            flags.blocked,
        ));

        if flags.blocked {
            if flags.readable && !flags.writable {
                pipe.inner.wait_for_writer()?;
            } else if flags.writable && !flags.readable {
                pipe.inner.wait_for_reader()?;
            }
        }

        Ok(pipe)
    }

    pub fn get_pipe_size(&self) -> usize {
        self.inner.get_capacity()
    }

    pub fn set_pipe_size(&self, size: usize) -> SysResult<usize> {
        self.inner.set_capacity(size)
    }

    pub fn read_with_blocked(&self, buf: &mut [u8], blocked: bool) -> SysResult<usize> {
        self.inner.read(buf, blocked)
    }

    pub fn read_to_user_with_blocked(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        self.inner.read_to_user(ubuf, blocked)
    }

    pub fn write_with_blocked(&self, buf: &[u8], blocked: bool) -> SysResult<usize> {
        self.inner.write(buf, blocked)
    }

    pub fn write_from_user_with_blocked(&self, ubuf: &UAddrSpaceBuffer, blocked: bool) -> SysResult<usize> {
        self.inner.write_from_user(ubuf, blocked)
    }

    pub fn peek_with_blocked(&self, len: usize, blocked: bool) -> SysResult<Vec<u8>> {
        self.inner.peek(len, blocked)
    }

    pub fn is_same_pipe(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl FileOps for Pipe {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        if !self.readable {
            return Err(Errno::EBADF);
        }
        let blocked = *self.blocked.lock();
        self.read_with_blocked(buf, blocked)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        if !self.readable {
            return Err(Errno::EBADF);
        }
        let blocked = *self.blocked.lock();
        self.read_to_user_with_blocked(ubuf, blocked)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        if !self.writable {
            return Err(Errno::EBADF);
        }
        let blocked = *self.blocked.lock();
        self.write_with_blocked(buf, blocked)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        if !self.writable {
            return Err(Errno::EBADF);
        }
        let blocked = *self.blocked.lock();
        self.write_from_user_with_blocked(ubuf, blocked)
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: self.readable,
            writable: self.writable,
            blocked: *self.blocked.lock(),
            append: false,
            direct: false,
        }
    }

    fn writable(&self) -> bool {
        self.writable
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        const FIONREAD: usize = 0x541B;

        match request {
            FIONREAD => {
                let available = self.inner.read_available();
                let value = available.min(i32::MAX as usize) as i32;
                addrspace.copy_to_user(arg, value)?;
                Ok(0)
            }
            _ => Err(Errno::ENOTTY),
        }
    }

    fn fstat(&self) -> SysResult<FileStat> {
        if let Some(meta) = &self.meta {
            return meta.inode.fstat();
        }

        let mut kstat = FileStat::empty();
        kstat.st_mode = Mode::S_IFIFO.bits() as u32 | 0o666;
        kstat.st_nlink = 1;

        Ok(kstat)
    }

    fn fstatfs(&self) -> SysResult<Statfs> {
        let mut kstatfs = Statfs::default();
        kstatfs.f_type = 0x50495045; // "PIPE"
        Ok(kstatfs)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        self.meta.as_ref().map(|m| &m.inode)
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.meta.as_ref().map(|m| &m.dentry)
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let mut ready = FileEvent::empty();
        if self.readable
            && let Some(event) = self.inner.wait_event(waker, event & FileEvent::READ_READY, false)?
        {
            ready |= event;
        }
        if self.writable
            && let Some(event) = self.inner.wait_event(waker, event & FileEvent::WRITE_READY, true)?
        {
            ready |= event;
        }

        if ready.is_empty() { Ok(None) } else { Ok(Some(ready)) }
    }

    fn poll_event(&self, event: FileEvent) -> SysResult<Option<FileEvent>> {
        let mut ready = FileEvent::empty();
        if self.readable
            && let Some(event) = self.inner.poll_event(event & FileEvent::READ_READY, false)?
        {
            ready |= event;
        }
        if self.writable
            && let Some(event) = self.inner.poll_event(event & FileEvent::WRITE_READY, true)?
        {
            ready |= event;
        }

        if ready.is_empty() { Ok(None) } else { Ok(Some(ready)) }
    }

    fn wait_event_cancel(&self) {
        self.inner.wait_event_cancel();
    }

    fn epoll_notifier(&self) -> Option<Arc<EpollNotifier>> {
        if self.readable {
            Some(self.inner.epoll_notifier(false))
        } else if self.writable {
            Some(self.inner.epoll_notifier(true))
        } else {
            None
        }
    }

    fn epoll_notifiers(&self) -> Option<Vec<Arc<EpollNotifier>>> {
        let mut notifiers = Vec::new();
        if self.readable {
            notifiers.push(self.inner.epoll_notifier(false));
        }
        if self.writable {
            notifiers.push(self.inner.epoll_notifier(true));
        }
        if notifiers.is_empty() { None } else { Some(notifiers) }
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.blocked.lock() = flags.blocked;
    }

    fn type_name(&self) -> &'static str {
        "pipe"
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        if self.readable {
            self.inner.decrement_reader_count();
        }
        if self.writable {
            self.inner.decrement_writer_count();
        }
    }
}
