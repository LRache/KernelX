use alloc::sync::Arc;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::fs::{Dentry, InodeOps};
use crate::kernel::errno::SysResult;
use crate::kernel::event::{PollEvent, PollEventSet};
use crate::kernel::uapi::FileStat;

pub enum SeekWhence {
    BEG,
    CUR,
    END,
}

pub trait FileOps: DowncastSync {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize>;
    fn pread(&self, buf: &mut [u8], offset: usize) -> SysResult<usize>;
    fn write(&self, buf: &[u8]) -> SysResult<usize>;
    fn pwrite(&self, buf: &[u8], offset: usize) -> SysResult<usize>;

    fn readable(&self) -> bool;
    fn writable(&self) -> bool;

    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize>;
    fn ioctl(&self, request: usize, arg: usize) -> SysResult<usize>;
    fn fstat(&self) -> SysResult<FileStat>;
    fn fsync(&self) -> SysResult<()>;
    // fn get_dent(&self) -> SysResult<Option<DirResult>>;

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>>;
    fn get_dentry(&self) -> Option<&Arc<Dentry>>;

    fn poll(&self, _waker: usize, _event: PollEventSet) -> SysResult<Option<PollEvent>> {
        Ok(None)
    }
    fn poll_cancel(&self) {}

    fn type_name(&self) -> &'static str {
        "unknown"
    }
}

impl_downcast!(sync FileOps);
