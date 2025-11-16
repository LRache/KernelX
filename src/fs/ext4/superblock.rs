use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use ext4_rs::{BlockDevice, BLOCK_SIZE};

use crate::kernel::errno::SysResult;
use crate::kernel::uapi::Statfs;
use crate::fs::ext4::inode::Ext4Inode;
use crate::fs::filesystem::SuperBlockOps;
use crate::fs::InodeOps;
use crate::driver::BlockDriverOps;
use crate::kinfo;

use super::superblock_inner::SuperBlockInner;

pub struct Ext4SuperBlock {
    sno: u32,
    superblock: Arc<SuperBlockInner>,
}

struct Disk {
    pub driver: Arc<dyn BlockDriverOps>
}

impl BlockDevice for Disk {
    fn read_offset(&self, offset: usize) -> Vec<u8> {
        let mut buf = vec![0u8; BLOCK_SIZE as usize];
        if self.driver.read_at(offset, &mut buf).is_err() {
            panic!("Failed to read at offset {}", offset);
        }

        // kinfo!("Disk::read_offset offset={:x} buf={:?}", offset, &buf);
        
        buf
    }

    fn write_offset(&self, offset: usize, buf: &[u8]) {
        self.driver.write_at(offset, buf).expect("Failed to write at offset");
    }
}

unsafe impl Send for Disk {}
unsafe impl Sync for Disk {}

impl Ext4SuperBlock {
    pub fn new(sno: u32, driver: Arc<dyn BlockDriverOps>) -> SysResult<Arc<Self>> {
        let superblock = SuperBlockInner::open(Arc::new(Disk{ driver }));

        let buffer = vec![65; 4096];
        superblock.write_at(13, 0, &buffer).unwrap();
        superblock.write_at(13, 4096, &[66; 1]).unwrap();

        kinfo!("Wrote test data to ext4 image");

        unreachable!();

        Ok(Arc::new(Ext4SuperBlock {
            sno,
            // fs_handler: 0,
            superblock: Arc::new(superblock),
        }))
    }
}

unsafe impl Send for Ext4SuperBlock {}  
unsafe impl Sync for Ext4SuperBlock {}

impl SuperBlockOps for Ext4SuperBlock {
    fn get_inode(&self, ino: u32) -> SysResult<Box<dyn InodeOps>> {
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

    fn statfs(&self) -> SysResult<Statfs> {
        let statfs = Statfs {
            f_type: 0xEF53, // EXT4 magic number
            f_bsize: self.superblock.super_block.block_size() as u64,
            f_blocks: self.superblock.super_block.blocks_count() as u64,
            f_bfree: self.superblock.super_block.free_blocks_count() as u64,
            f_bavail: self.superblock.super_block.free_blocks_count() as u64,
            f_files: self.superblock.super_block.total_inodes() as u64,
            f_ffree: self.superblock.super_block.free_inodes_count() as u64,
            f_fsid: 0,
            f_namelen: 255,
            f_frsize: self.superblock.super_block.block_size() as u64,
            f_flag: 0,
            f_spare: [0; 4],
        };
        Ok(statfs)
    }
}
