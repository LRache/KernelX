use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use ext4_rs::{BlockDevice, BLOCK_SIZE};

use crate::kernel::errno::SysResult;
use crate::fs::ext4::inode::Ext4Inode;
use crate::fs::filesystem::SuperBlock;
use crate::fs::inode::Inode;
use crate::driver::block::BlockDriver;

use super::superblock_inner::SuperBlockInner;

pub struct Ext4SuperBlock {
    sno: u32,
    superblock: Arc<SuperBlockInner>,
}

struct Disk {
    pub driver: Box<dyn BlockDriver>
}

impl BlockDevice for Disk {
    fn read_offset(&self, offset: usize) -> Vec<u8> {
        let mut buf = vec![0u8; BLOCK_SIZE as usize];
        self.driver.read_at(offset, &mut buf).expect("Failed to read at offset");
        
        buf
    }

    fn write_offset(&self, offset: usize, buf: &[u8]) {
        self.driver.write_at(offset, buf).expect("Failed to write at offset");
    }
}

unsafe impl Send for Disk {}
unsafe impl Sync for Disk {}

impl Ext4SuperBlock {
    pub fn new(sno: u32, driver: Box<dyn BlockDriver>) -> SysResult<Arc<Self>> {
        let superblock = SuperBlockInner::open(Arc::new(Disk{ driver }));

        Ok(Arc::new(Ext4SuperBlock {
            sno,
            // fs_handler: 0,
            superblock: Arc::new(superblock),
        }))
    }
}

unsafe impl Send for Ext4SuperBlock {}  
unsafe impl Sync for Ext4SuperBlock {}

impl SuperBlock for Ext4SuperBlock {
    fn get_inode(&self, ino: u32) -> SysResult<Box<dyn Inode>> {
        // Ok(Box::new(Ext4Inode::new(ino as u32, self.sno, self.fs_handler)?))
        Ok(Box::new(Ext4Inode::new(self.sno, ino, &self.superblock)))
    }

    fn get_root_ino(&self) -> u32 {
        2
    }

    fn unmount(&self) -> SysResult<()> {
        // destroy_filesystem(self.fs_handler)
        Ok(())
    }
}
