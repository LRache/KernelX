use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::kernel::errno::SysResult;
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use crate::fs::InodeOps;
use crate::driver::BlockDriverOps;

use super::{root, null, zero};
use super::def::*;

pub struct DevFileSystem;

impl FileSystemOps for DevFileSystem {
    fn create(&self, sno: u32, _driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<Arc<dyn SuperBlockOps>> {
        return Ok(DevSuperBlock::new(sno) as Arc<dyn SuperBlockOps>);
    }
}

struct DevSuperBlock {
    sno: u32,
}

impl DevSuperBlock {
    pub fn new(sno: u32) -> Arc<Self> {
        Arc::new(DevSuperBlock { sno })
    }
}

impl SuperBlockOps for DevSuperBlock {
    fn get_root_ino(&self) -> u32 {
        ROOT_INO
    }
    
    fn get_inode(&self, ino: u32) -> SysResult<Box<dyn InodeOps>> {
        match ino {
            ROOT_INO => Ok(Box::new(root::RootInode::new(self.sno))),
            NULL_INO => Ok(Box::new(null::NullInode::new(self.sno))),
            ZERO_INO => Ok(Box::new(zero::ZeroInode::new(self.sno))),
            _ => unreachable!("DevFS only has 3 inodes"),
        }
    }

    fn unmount(&self) -> SysResult<()> {
        Ok(())
    }
}
