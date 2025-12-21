use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::kernel::errno::{SysResult, Errno};
use crate::fs::{Mode, InodeOps};
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};

use super::inode;

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(&self, _fsno: u32, _driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<Arc<dyn SuperBlockOps>> {
        Ok(Arc::new(SuperBlock))
    }
}

pub struct SuperBlock;

impl SuperBlockOps for SuperBlock {
    fn get_root_ino(&self) -> u32 {
        inode::RootInode::INO
    }

    fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        match ino {
            inode::RootInode::INO => Ok(Arc::new(inode::RootInode)),
            inode::TaskDirSelfInode::INO => Ok(Arc::new(inode::TaskDirSelfInode)),
            i if i >= inode::TaskDirInode::BASE_INO && i < inode::TaskMapsInode::INO_BASE => {
                Ok(Arc::new(inode::TaskDirInode::from_ino(i).ok_or(Errno::ENOENT)?))
            }
            i if i >= inode::TaskMapsInode::INO_BASE => {
                Ok(Arc::new(inode::TaskMapsInode::from_ino(i).ok_or(Errno::ENOENT)?))
            }
            _ => Err(Errno::ENOENT),
        }
    }
    
    fn create_temp(&self, _mode: Mode) -> SysResult<Arc<dyn InodeOps>> {
        Err(Errno::EROFS)
    }

    fn sync(&self) -> SysResult<()> {
        Ok(())
    }
}
