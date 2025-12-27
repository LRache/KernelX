use core::time::Duration;

use alloc::sync::Arc;
use lwext4_rust::{BlockDevice, Ext4Error, Ext4Filesystem, Ext4Result, FsConfig, SystemHal};
use lwext4_rust::EXT4_DEV_BSIZE;

use crate::driver::chosen::kclock;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::Statfs;
use crate::klib::SpinLock;
use crate::fs::ext4::inode::Ext4Inode;
use crate::fs::filesystem::SuperBlockOps;
use crate::fs::InodeOps;
use crate::driver::BlockDriverOps;

pub(super) fn map_error_to_ext4(e: Errno, context: &'static str) -> Ext4Error {
    Ext4Error { 
        code: e as i32, 
        context: Some(context)
    }
}

pub(super) fn map_error_to_kernel(e: Ext4Error) -> Errno {
    Errno::try_from(e.code).expect("unexpected code")
}

pub(super) struct BlockDeviceImpls {
    driver: Arc<dyn BlockDriverOps>
}

impl BlockDeviceImpls {
    fn new(driver: Arc<dyn BlockDriverOps>) -> Self {
        Self { driver }
    }
}

impl BlockDevice for BlockDeviceImpls {
    fn num_blocks(&self) -> Ext4Result<u64> {
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

pub(super) struct SystemHalImpls;

impl SystemHal for SystemHalImpls {
    fn now() -> Option<Duration> {
        kclock::now().ok()
    }
}

pub(super) type SuperBlockInner = Ext4Filesystem<SystemHalImpls, BlockDeviceImpls>;

pub struct Ext4SuperBlock {
    superblock: Arc<SpinLock<SuperBlockInner>>
}

impl Ext4SuperBlock {
    pub fn new(driver: Arc<dyn BlockDriverOps>) -> SysResult<Arc<Self>> {
        let superblock = Ext4Filesystem::new(BlockDeviceImpls::new(driver), FsConfig::default()).map_err(map_error_to_kernel)?;

        Ok(Arc::new(Self { 
            superblock: Arc::new(SpinLock::new(superblock))
        }))
    }
}

unsafe impl Send for Ext4SuperBlock {}  
unsafe impl Sync for Ext4SuperBlock {}

impl SuperBlockOps for Ext4SuperBlock {
    fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        Ok(Arc::new(Ext4Inode::new(ino, self.superblock.clone())))
    }

    fn get_root_ino(&self) -> u32 {
        2
    }

    fn unmount(&self) -> SysResult<()> {
        Ok(())
    }

    fn statfs(&self) -> SysResult<Statfs> {
        let stat = self.superblock.lock().stat().map_err(map_error_to_kernel)?;
        let statfs = Statfs {
            f_type: 0xEF53, // EXT4 magic number
            f_bsize: stat.block_size as u64,
            f_blocks: stat.blocks_count as u64,
            f_bfree: stat.free_blocks_count as u64,
            f_bavail: stat.free_blocks_count as u64,
            f_files: stat.inodes_count as u64,
            f_ffree: stat.free_inodes_count as u64,
            f_fsid: 0,
            f_namelen: 255,
            f_frsize: stat.block_size as u64,
            f_flag: 0,
            f_spare: [0; 4],
        };
        Ok(statfs)
    }

    fn sync(&self) -> SysResult<()> {
        self.superblock.lock().flush().map_err(map_error_to_kernel)
    }
}
