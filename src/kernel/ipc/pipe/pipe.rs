use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::FileEvent;
use crate::kernel::mm::AddrSpace;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

use super::PipeInner;

struct Meta {
    // inode: Arc<dyn InodeOps>,
    dentry: Arc<Dentry>,
}

pub struct Pipe {
    inner: Arc<PipeInner>,
    meta: Option<Meta>,
    writable: bool,
    blocked: SpinLock<bool>,
}

impl Pipe {
    pub fn new(inner: Arc<PipeInner>, writable: bool, blocked: bool) -> Self {
        if writable {
            inner.increment_writer_count();
        } else {
            inner.increment_reader_count();
        }
        Self {
            inner,
            meta: None,
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
        self.read_with_blocked(buf, *self.blocked.lock())
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        self.read_to_user_with_blocked(ubuf, *self.blocked.lock())
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.write_with_blocked(buf, *self.blocked.lock())
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        self.write_from_user_with_blocked(ubuf, *self.blocked.lock())
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: !self.writable,
            writable: self.writable,
            blocked: *self.blocked.lock(),
            append: false,
            direct: false,
        }
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
        let mut kstat = FileStat::empty();
        kstat.st_mode = Mode::S_IFIFO.bits() as u32 | 0o666;
        kstat.st_nlink = 1;

        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        // self.meta.as_ref().map(|m| &m.inode)
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.meta.as_ref().map(|m| &m.dentry)
    }

    fn wait_event(&self, waker: usize, event: FileEvent) -> SysResult<Option<FileEvent>> {
        self.inner.wait_event(waker, event, self.writable)
    }

    fn wait_event_cancel(&self) {
        self.inner.wait_event_cancel();
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
        if self.writable {
            self.inner.decrement_writer_count();
        } else {
            self.inner.decrement_reader_count();
        }
    }
}
