use alloc::sync::Arc;

use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::uapi::FileStat;
use crate::kernel::event::{PollEvent, PollEventSet};
use crate::driver::CharDriverOps;
use crate::fs::file::FileOps;

use super::{SeekWhence, DirResult};

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

    fn write(&self, buf: &[u8]) -> crate::kernel::errno::SysResult<usize> {
        for &c in buf {
            self.driver.putchar(c);
        }
        Ok(buf.len())
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

    fn get_dent(&self) -> SysResult<Option<DirResult>> {
        Err(Errno::ENOTDIR)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        Ok(FileStat::empty())
    }

    fn get_dentry(&self) -> Option<&Arc<crate::fs::Dentry>> {
        None
    }

    fn get_inode(&self) -> Option<&Arc<dyn crate::fs::Inode>> {
        None
    }

    fn ioctl(&self, _request: usize, _arg: usize) -> SysResult<usize> {
        Err(Errno::ENOSYS)
    }

    fn poll(&self, waker: usize, event: PollEventSet) -> SysResult<Option<PollEvent>> {
        self.driver.poll(waker, event)
    }

    fn type_name(&self) -> &'static str {
        "CharFile"
    }
}

unsafe impl Send for CharFile {}
unsafe impl Sync for CharFile {}
