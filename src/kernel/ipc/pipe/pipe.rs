use alloc::sync::Arc;

use crate::fs::file::{FileOps, SeekWhence};
use crate::fs::{Dentry, InodeOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{PollEvent, PollEventSet};
use crate::kernel::uapi::FileStat;

use super::PipeInner;

struct Meta {
    // inode: Arc<dyn InodeOps>,
    dentry: Arc<Dentry>,
}

pub struct Pipe {
    inner: Arc<PipeInner>,
    meta: Option<Meta>,
    writable: bool,
    blocked: bool,
}

impl Pipe {
    pub fn new(inner: Arc<PipeInner>, writable: bool) -> Self {
        if writable {
            inner.increment_writer_count();
        }
        Self {
            inner,
            meta: None,
            writable,
            blocked: true,
        }
    }

    pub fn create(capacity: usize) -> (Self, Self) {
        let inner = Arc::new(PipeInner::new(capacity));
        let read_end = Pipe::new(inner.clone(), false);
        let write_end = Pipe::new(inner, true);
        (read_end, write_end)
    }
}

impl FileOps for Pipe {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        self.inner.read(buf)
    }

    fn pread(&self, _: &mut [u8], _: usize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.inner.write(buf)
    }

    fn pwrite(&self, _: &[u8], _: usize) -> SysResult<usize> {
        Err(Errno::EPIPE)
    }

    fn readable(&self) -> bool {
        !self.writable
    }

    fn writable(&self) -> bool {
        self.writable
    }

    fn seek(&self, _: isize, _: SeekWhence) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_mode = 0o100666; // Regular file with rw-rw-rw- permissions
        kstat.st_nlink = 1;

        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn ioctl(&self, _request: usize, _arg: usize) -> SysResult<usize> {
        Err(Errno::ENOSYS) // Placeholder for unimplemented ioctl commands
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        // self.meta.as_ref().map(|m| &m.inode)
        unimplemented!()
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.meta.as_ref().map(|m| &m.dentry)
    }

    fn poll(&self, waker: usize, event: PollEventSet) -> SysResult<Option<PollEvent>> {
        self.inner.poll(waker, event, self.writable)
    }

    fn poll_cancel(&self) {
        self.inner.poll_cancel();
    }

    fn type_name(&self) -> &'static str {
        "pipe"
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        if self.writable {
            self.inner.decrement_writer_count();
        }
    }
}
