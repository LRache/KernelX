use alloc::sync::Arc;

use crate::kernel::errno::Errno;
use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystem, SuperBlock};
use super::superblock::Ext4SuperBlock;

pub struct Ext4FileSystem;

impl Ext4FileSystem {
    pub fn new() -> Self {
        Ext4FileSystem {}
    }
}

impl FileSystem for Ext4FileSystem {
    fn create(&self, sno: u32, driver: Option<Arc<dyn BlockDriverOps>>) -> Result<Arc<dyn SuperBlock>, Errno> {
        Ok(Ext4SuperBlock::new(sno, driver.unwrap())?)
    }
}
