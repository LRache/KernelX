use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
// use ext4_rs::{BlockDevice, BLOCK_SIZE};
use lwext4_rust::{BlockDevice, DummyHal, Ext4Error, Ext4Filesystem, Ext4Result, FsConfig};
use lwext4_rust::EXT4_DEV_BSIZE;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::Statfs;
use crate::fs::ext4::inode::Ext4Inode;
use crate::fs::filesystem::SuperBlockOps;
use crate::fs::InodeOps;
use crate::driver::BlockDriverOps;
use crate::kwarn;

use super::superblock_inner::SuperBlockInner;

fn map_error_to_ext4(e: Errno, context: &'static str) -> Ext4Error {
    Ext4Error { 
        code: e as i32, 
        context: Some(context)
    }
}

fn map_error_to_kernel(e: Ext4Error) -> Errno {
    if e.context.is_some() {
        kwarn!("{:?}", e);
    }

    Errno::try_from(e.code).expect("unexpected code")
}

struct BlockDeviceImpls {
    driver: Arc<dyn BlockDriverOps>
}

impl BlockDeviceImpls {
    fn new(driver: Arc<dyn BlockDriverOps>) -> Self {
        Self { driver }
    }
}

impl BlockDevice for BlockDeviceImpls {
    fn num_blocks(&self) -> lwext4_rust::Ext4Result<u64> {
        Ext4Result::Ok(self.driver.get_block_size() as u64 * self.driver.get_block_count() / EXT4_DEV_BSIZE as u64)
    }

    fn read_blocks(&mut self, block_id: u64, buf: &mut [u8]) -> Ext4Result<usize> {
        self.driver.read_at(block_id as usize * EXT4_DEV_BSIZE as usize, buf).map_err(|_| map_error_to_ext4(Errno::EIO, "read_block"))?;
        Ext4Result::Ok(buf.len())
    }

    fn write_blocks(&mut self, block_id: u64, buf: &[u8]) -> Ext4Result<usize> {
        self.driver.write_at(block_id as usize * EXT4_DEV_BSIZE, buf).map_err(|_| map_error_to_ext4(Errno::EIO, "write_block"))?;
        Ok(buf.len())
    }
}

pub struct Ext4SuperBlock {
    sno: u32,
    // superblock: Arc<SuperBlockInner>,
    superblock: Ext4Filesystem<DummyHal, BlockDeviceImpls>
}

// struct Disk {
//     pub driver: Arc<dyn BlockDriverOps>
// }

// impl BlockDevice for Disk {
//     fn read_offset(&self, offset: usize) -> Vec<u8> {
//         let mut buf = vec![0u8; BLOCK_SIZE as usize];
//         if self.driver.read_at(offset, &mut buf).is_err() {
//             panic!("Failed to read at offset {}", offset);
//         }

//         // kinfo!("Disk::read_offset offset={:x} buf={:?}", offset, &buf);
        
//         buf
//     }

//     fn write_offset(&self, offset: usize, buf: &[u8]) {
//         self.driver.write_at(offset, buf).expect("Failed to write at offset");
//     }
// }

// unsafe impl Send for Disk {}
// unsafe impl Sync for Disk {}

impl Ext4SuperBlock {
    pub fn new(sno: u32, driver: Arc<dyn BlockDriverOps>) -> SysResult<Arc<Self>> {
        // let superblock = SuperBlockInner::open(Arc::new(Disk{ driver }));

        // Ok(Arc::new(Ext4SuperBlock {
        //     sno,
        //     superblock: Arc::new(superblock),
        // }))
        let superblock = Ext4Filesystem::new(BlockDeviceImpls::new(driver), FsConfig::default()).map_err(map);

        Ok(Arc::new(Self { sno, superblock }))
    }
}

unsafe impl Send for Ext4SuperBlock {}  
unsafe impl Sync for Ext4SuperBlock {}

impl SuperBlockOps for Ext4SuperBlock {
    fn get_inode(&self, ino: u32) -> SysResult<Box<dyn InodeOps>> {
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
