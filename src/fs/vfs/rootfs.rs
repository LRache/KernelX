use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::fs::filesystem::{FileSystem, SuperBlock};
use crate::fs::Inode;
use crate::fs::inode::InodeNumber;
use crate::driver::block::BlockDevice;
use crate::kernel::errno::Errno;

#[derive(Debug, Clone)]
pub struct RootInode {}

unsafe impl Send for RootInode {}
unsafe impl Sync for RootInode {}

impl Inode for RootInode {
    fn get_ino(&self) -> InodeNumber {
        0
    }

    fn get_fsno(&self) -> usize {
        0
    }

    fn readat(&mut self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn writeat(&mut self, _buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn lookup(&self, _name: &str) -> Result<InodeNumber, Errno> {
        Err(Errno::ENOENT)
    }
}

pub struct RootFileSystem;

impl RootFileSystem {
    pub fn new() -> Self {
        RootFileSystem {}
    }
}

pub struct RootFileSystemSuperBlock;

impl RootFileSystemSuperBlock {
    pub const fn new() -> Self {
        RootFileSystemSuperBlock {}
    }
}

impl SuperBlock for RootFileSystemSuperBlock {
    fn get_root_inode(&self) -> Box<dyn Inode> {
        Box::new(RootInode {})
    }

    fn get_inode(&self, _ino: usize) -> Result<Box<dyn Inode>, Errno> {
        Ok(Box::new(RootInode {}))
    }
}

impl FileSystem for RootFileSystem {
    fn create(&self, _fsno: usize, _device: Option<Box<dyn BlockDevice>>) -> Result<Arc<dyn SuperBlock>, Errno> {
        Ok(Arc::new(RootFileSystemSuperBlock::new()))
    }
}
