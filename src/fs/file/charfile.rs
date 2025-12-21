use alloc::sync::Arc;

use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::mm::AddrSpace;
use crate::kernel::uapi::FileStat;
use crate::kernel::event::{FileEvent, PollEventSet};
use crate::driver::CharDriverOps;
use crate::fs::file::{FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps};

use super::SeekWhence;

pub struct CharFile {
    driver: Arc<dyn CharDriverOps>,
    inode: Arc<dyn InodeOps>,
    dentry: Option<Arc<Dentry>>,
    readable: bool,
    writable: bool,
    blocked: bool,
}

impl CharFile {
    pub fn new(driver: Arc<dyn CharDriverOps>, inode: Arc<dyn InodeOps>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Self {
        CharFile { 
            driver, 
            inode, 
            dentry,
            readable: flags.readable,
            writable: flags.writable,
            blocked: flags.blocked
        }
    }
}

impl FileOps for CharFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        self.driver.read(buf, self.blocked)
    }

    fn pread(&self, _: &mut [u8], _: usize) -> SysResult<usize> {
        Err(Errno::EPIPE)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.driver.write(buf)
    }

    fn pwrite(&self, _: &[u8], _: usize) -> SysResult<usize> {
        Err(Errno::EPIPE)
    }

    fn readable(&self) -> bool {
        self.readable
    }

    fn writable(&self) -> bool {
        self.writable
    }

    fn seek(&self, _offset: isize, _whence: SeekWhence) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inode.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        self.dentry.as_ref()
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        Some(&self.inode)
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        self.driver.ioctl(request, arg, addrspace)
    }

    fn wait_event(&self, waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        self.driver.wait_event(waker, event)
    }

    fn wait_event_cancel(&self) {
        self.driver.wait_event_cancel();
    }

    fn type_name(&self) -> &'static str {
        "CharFile"
    }
}

unsafe impl Send for CharFile {}
unsafe impl Sync for CharFile {}
