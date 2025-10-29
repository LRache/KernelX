use alloc::sync::Arc;

use crate::kernel::errno::Errno;
use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use super::superblock::Ext4SuperBlock;

pub struct Ext4FileSystem;

impl FileSystemOps for Ext4FileSystem {
    fn create(&self, sno: u32, driver: Option<Arc<dyn BlockDriverOps>>) -> Result<Arc<dyn SuperBlockOps>, Errno> {
        Ok(Ext4SuperBlock::new(sno, driver.unwrap())?)
    }
}
