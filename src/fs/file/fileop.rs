use alloc::sync::Arc;
use downcast_rs::{DowncastSync, impl_downcast};

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
    
    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize>;
    fn ioctl(&self, request: usize, arg: usize) -> SysResult<usize>;
    fn fstat(&self) -> SysResult<FileStat>;
    fn get_dent(&self) -> SysResult<Option<DirResult>>;
    fn get_inode(&self) -> SysResult<&Arc<LockedInode>>;
    fn get_dentry(&self) -> SysResult<&Arc<Dentry>>;
}

impl_downcast!(sync FileOps);
