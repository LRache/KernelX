use alloc::sync::Arc;

use super::superblock::Ext4SuperBlock;
use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps};
use crate::kernel::errno::Errno;

pub struct Ext4FileSystem;

impl FileSystemOps for Ext4FileSystem {
    fn create(
        &self,
        _: u32,
        driver: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> Result<Arc<dyn SuperBlockOps>, Errno> {
        let driver = driver.ok_or(Errno::ENODEV)?;
        if driver.is_readonly() && !options.read_only {
            return Err(Errno::EACCES);
        }
        Ok(Ext4SuperBlock::new(driver, options.read_only)?)
    }
}
