use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::kernel::errno::Errno;
use crate::driver::block::BlockDevice;
use crate::fs::filesystem::{FileSystem, SuperBlock};
use super::superblock::Ext4SuperBlock;

pub struct Ext4FileSystem;

impl Ext4FileSystem {
    pub fn new() -> Self {
        Ext4FileSystem {}
    }
}

impl FileSystem for Ext4FileSystem {
    fn create(&self, fsno: usize, device: Option<Box<dyn BlockDevice>>) -> Result<Arc<dyn SuperBlock>, Errno> {
        Ok(Ext4SuperBlock::new(fsno, device.unwrap())?)
    }
}
