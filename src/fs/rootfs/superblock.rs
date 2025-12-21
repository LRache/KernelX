use alloc::sync::Arc;

use crate::fs::file::{FileFlags, FileOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use crate::fs::{Dentry, InodeOps};
use crate::driver::BlockDriverOps;

#[derive(Debug, Clone)]
pub struct RootInode;

unsafe impl Send for RootInode {}
unsafe impl Sync for RootInode {}

impl RootInode {
    pub const fn new() -> Self {
        RootInode
    }
}

impl InodeOps for RootInode {
    fn get_ino(&self) -> u32 {
        0
    }

    fn type_name(&self) -> &'static str {
        "rootfs"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn lookup(&self, _name: &str) -> Result<u32, Errno> {
        Err(Errno::ENOENT)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, _: Option<Arc<Dentry>>, _: FileFlags) -> Arc<dyn FileOps> {
        unreachable!()
    }
}

pub struct RootFileSystem;

pub struct RootFileSystemSuperBlock;

impl RootFileSystemSuperBlock {
    pub const fn new() -> Self {
        RootFileSystemSuperBlock {}
    }
}

impl SuperBlockOps for RootFileSystemSuperBlock {
    fn get_root_ino(&self) -> u32 {
        0
    }

    fn get_inode(&self, _ino: u32) -> Result<Arc<dyn InodeOps>, Errno> {
        Ok(Arc::new(RootInode::new()))
    }
}

impl FileSystemOps for RootFileSystem {
    fn create(&self, _fsno: u32, _driver: Option<Arc<dyn BlockDriverOps>>) -> Result<Arc<dyn SuperBlockOps>, Errno> {
        Ok(Arc::new(RootFileSystemSuperBlock::new()))
    }
}
