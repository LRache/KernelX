use alloc::sync::Arc;

use super::superblock::Ext4SuperBlock;
use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use crate::kernel::errno::{Errno, SysResult};

pub struct Ext4FileSystem;

impl FileSystemOps for Ext4FileSystem {
    fn create(&self, _: u32, driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<Arc<dyn SuperBlockOps>> {
        let driver = driver.ok_or(Errno::EINVAL)?;
        Ok(Ext4SuperBlock::new(driver)?)
    }
}
