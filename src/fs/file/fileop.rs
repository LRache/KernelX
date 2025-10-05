use alloc::sync::Arc;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::kernel::event::{Event, PollEventSet};
use crate::kernel::errno::SysResult;
use crate::fs::{Dentry, LockedInode};

use super::{FileStat, DirResult};

pub enum SeekWhence {
    BEG,
    CUR,
    END,
}

pub trait FileOps: DowncastSync {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize>;
    fn write(&self, buf: &[u8]) -> SysResult<usize>;

    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    
    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize>;
    fn ioctl(&self, request: usize, arg: usize) -> SysResult<usize>;
    fn fstat(&self) -> SysResult<FileStat>;
    fn get_dent(&self) -> SysResult<Option<DirResult>>;
    
    fn get_inode(&self) -> Option<&Arc<LockedInode>>;
    fn get_dentry(&self) -> Option<&Arc<Dentry>>;

    fn poll(&self, _waker: usize, _event: PollEventSet) -> SysResult<Option<Event>> {
        Ok(None)
    }

    fn type_name(&self) -> &'static str {
        "unknown"
    }
}

impl_downcast!(sync FileOps);
