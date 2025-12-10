use alloc::sync::Arc;

use crate::kernel::event::{FileEvent, PollEventSet};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;
use crate::fs::{Dentry, InodeOps};
use crate::fs::file::{FileOps, SeekWhence};

use super::PipeInner;

struct Meta {
    // inode: Arc<dyn InodeOps>,
    dentry: Arc<Dentry>,
}

pub struct Pipe {
    inner: Arc<PipeInner>,
    meta: Option<Meta>,
    writable: bool,
    _blocked: bool,
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
            _blocked: true,
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

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        // self.meta.as_ref().map(|m| &m.inode)
        unimplemented!()
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.meta.as_ref().map(|m| &m.dentry)
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        self.inner.wait_event(waker, event, self.writable)
    }

    fn wait_event_cancel(&self, ) {
        self.inner.wait_event_cancel();
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
