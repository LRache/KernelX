use alloc::sync::Arc;

use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::mm::AddrSpace;
use crate::kernel::uapi::FileStat;
use crate::kernel::event::{FileEvent, PollEventSet};
use crate::driver::CharDriverOps;
use crate::fs::file::FileOps;
use crate::fs::{InodeOps, Mode};

use super::SeekWhence;

pub struct CharFile {
    driver: Arc<dyn CharDriverOps>,
}

impl CharFile {
    pub fn new(driver: Arc<dyn CharDriverOps>) -> Self {
        CharFile { driver }
    }
}

impl FileOps for CharFile {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let mut count = 0;
        for byte in buf.iter_mut() {
            if let Some(c) = self.driver.getchar() {
                *byte = c;
                count += 1;
            } else {
                break;
            }
        }
        Ok(count)
    }

    fn pread(&self, _: &mut [u8], _: usize) -> SysResult<usize> {
        Err(Errno::EPIPE)
    }

    fn write(&self, buf: &[u8]) -> crate::kernel::errno::SysResult<usize> {
        for &c in buf {
            self.driver.putchar(c);
        }
        Ok(buf.len())
    }

    fn pwrite(&self, _: &[u8], _: usize) -> SysResult<usize> {
        Err(Errno::EPIPE)
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn seek(&self, _offset: isize, _whence: SeekWhence) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();

        kstat.st_mode = Mode::S_IFCHR.bits() as u32;

        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_dentry(&self) -> Option<&Arc<crate::fs::Dentry>> {
        None
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
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
