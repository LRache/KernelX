use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::fs::filesystem::{FileSystem, SuperBlock};
use crate::fs::Inode;
use crate::driver::block::BlockDriver;
use crate::kernel::errno::Errno;

#[derive(Debug, Clone)]
pub struct RootInode {}

unsafe impl Send for RootInode {}
unsafe impl Sync for RootInode {}

impl RootInode {
    pub const fn new() -> Self {
        RootInode {}
    }
}

impl Inode for RootInode {
    fn get_ino(&self) -> u32 {
        0
    }

    fn get_sno(&self) -> u32 {
        0
    }

    fn type_name(&self) -> &'static str {
        "rootfs"
    }

    fn readat(&mut self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn writeat(&mut self, _buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        Ok(0)
    }

    fn lookup(&mut self, _name: &str) -> Result<u32, Errno> {
        Err(Errno::ENOENT)
    }
}

pub struct RootFileSystem;

impl RootFileSystem {
    pub fn new() -> Box<dyn FileSystem> {
        Box::new(RootFileSystem {})
    }
}

pub struct RootFileSystemSuperBlock;

impl RootFileSystemSuperBlock {
    pub const fn new() -> Self {
        RootFileSystemSuperBlock {}
    }
}

impl SuperBlock for RootFileSystemSuperBlock {
    fn get_root_ino(&self) -> u32 {
        0
    }

    fn get_inode(&self, _ino: u32) -> Result<Box<dyn Inode>, Errno> {
        Ok(Box::new(RootInode {}))
    }
}

impl FileSystem for RootFileSystem {
    fn create(&self, _fsno: u32, _driver: Option<Box<dyn BlockDriver>>) -> Result<Arc<dyn SuperBlock>, Errno> {
        Ok(Arc::new(RootFileSystemSuperBlock::new()))
    }
}
