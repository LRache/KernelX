use alloc::sync::Arc;

use crate::kernel::event::{Event, PollEventSet};
use crate::kernel::errno::{Errno, SysResult};
use crate::fs::{Dentry, LockedInode};
use crate::fs::file::{FileOps, FileStat, SeekWhence, DirResult};
use crate::kinfo;

use super::PipeInner;

struct Meta {
    inode: Arc<LockedInode>,
    dentry: Arc<Dentry>,
}

pub struct Pipe {
    inner: Arc<PipeInner>,
    meta: Option<Meta>,
    writable: bool,
}

impl Pipe {
    pub fn new(inner: Arc<PipeInner>, writable: bool) -> Self {
        if writable {
            inner.increment_writer_count();
        }
        Self {
            inner,
            meta: None,
            writable
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

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.inner.write(buf)
    }

    fn readable(&self) -> bool {
        !self.writable
    }

    fn writable(&self) -> bool {
        self.writable
    }

    fn seek(&self, _offset: isize, _whence: SeekWhence) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::new();
        kstat.st_mode = 0o100666; // Regular file with rw-rw-rw- permissions
        kstat.st_nlink = 1;

        Ok(kstat)
    }

    fn ioctl(&self, _request: usize, _arg: usize) -> SysResult<usize> {
        Err(Errno::ENOSYS) // Placeholder for unimplemented ioctl commands
    }

    fn get_dent(&self) -> SysResult<Option<DirResult>> {
        Err(Errno::ENOTDIR)
    }

    fn get_inode(&self) -> Option<&Arc<LockedInode>> {
        self.meta.as_ref().map(|m| &m.inode)
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.meta.as_ref().map(|m| &m.dentry)
    }

    fn poll(&self, waker: usize, event: PollEventSet) -> SysResult<Option<Event>> {
        self.inner.poll(waker, event, self.writable)
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
