use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::driver::BlockDriverOps;
use crate::fs::InodeOps;
use crate::fs::ext4::inode::Ext4Inode;
use crate::fs::ext4::ondisk::{Ext4FeatureSet, Ext4SuperBlockDisk, load_superblock, validate_feature_set};
use crate::fs::filesystem::SuperBlockOps;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::Statfs;

pub(super) struct Ext4RuntimeState {
    pub driver: Arc<dyn BlockDriverOps>,
    pub sb: Ext4SuperBlockDisk,
    pub feature_set: Ext4FeatureSet,
    pub readonly: bool,
}

pub struct Ext4SuperBlock {
    state: Arc<Ext4RuntimeState>,
}

impl Ext4SuperBlock {
    pub fn new(driver: Arc<dyn BlockDriverOps>) -> SysResult<Arc<Self>> {
        let sb = load_superblock(&*driver)?;
        validate_feature_set(&sb.features)?;

        // Phase-0: metadata preflight only. Recovery and RW paths are introduced later.
        let readonly = sb.features.requires_readonly || sb.features.needs_recovery;
        let feature_set = sb.features;

        Ok(Arc::new(Self {
            state: Arc::new(Ext4RuntimeState {
                driver,
                sb,
                feature_set,
                readonly,
            }),
        }))
    }
}

unsafe impl Send for Ext4SuperBlock {}
unsafe impl Sync for Ext4SuperBlock {}

impl Ext4RuntimeState {
    pub fn block_size(&self) -> u32 {
        self.sb.block_size
    }

    pub fn block_size_usize(&self) -> SysResult<usize> {
        usize::try_from(self.block_size()).map_err(|_| Errno::EINVAL)
    }

    pub fn read_bytes(&self, offset: usize, buf: &mut [u8]) -> SysResult<()> {
        self.driver.read_at(offset, buf).map_err(|_| Errno::EIO)
    }

    pub fn write_bytes(&self, offset: usize, buf: &[u8]) -> SysResult<()> {
        self.driver.write_at(offset, buf).map_err(|_| Errno::EIO)
    }

    pub fn read_block(&self, block_id: u64, buf: &mut [u8]) -> SysResult<()> {
        let bs = self.block_size_usize()?;
        if buf.len() != bs {
            return Err(Errno::EINVAL);
        }
        let byte_off = (block_id as usize).checked_mul(bs).ok_or(Errno::EINVAL)?;
        self.read_bytes(byte_off, buf)
    }

    pub fn read_block_alloc(&self, block_id: u64) -> SysResult<Vec<u8>> {
        let bs = self.block_size_usize()?;
        let mut block = vec![0u8; bs];
        self.read_block(block_id, &mut block)?;
        Ok(block)
    }

    pub fn group_count(&self) -> u32 {
        self.sb.inodes_count.div_ceil(self.sb.inodes_per_group)
    }

    pub fn read_group_desc_inode_table_block(&self, group: u32) -> SysResult<u64> {
        if group >= self.group_count() {
            return Err(Errno::EINVAL);
        }

        let bs = self.block_size_usize()?;
        let gdt_start_block = if self.sb.block_size == 1024 { 2u64 } else { 1u64 };
        let gdt_start = (gdt_start_block as usize).checked_mul(bs).ok_or(Errno::EINVAL)?;
        let desc_size = self.sb.desc_size as usize;
        if desc_size < 32 {
            return Err(Errno::EINVAL);
        }

        let desc_off = gdt_start
            .checked_add((group as usize).checked_mul(desc_size).ok_or(Errno::EINVAL)?)
            .ok_or(Errno::EINVAL)?;

        let mut desc = vec![0u8; desc_size];
        self.read_bytes(desc_off, &mut desc)?;

        let inode_table_lo = u32::from_le_bytes([desc[8], desc[9], desc[10], desc[11]]) as u64;
        let inode_table_hi = if self.sb.desc_size >= 64 {
            u32::from_le_bytes([desc[40], desc[41], desc[42], desc[43]]) as u64
        } else {
            0
        };
        Ok(inode_table_lo | (inode_table_hi << 32))
    }
}

impl SuperBlockOps for Ext4SuperBlock {
    fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>> {
        Ok(Arc::new(Ext4Inode::new(ino, self.state.clone())))
    }

    fn get_root_ino(&self) -> u32 {
        2
    }

    fn unmount(&self) -> SysResult<()> {
        self.state.driver.flush().map_err(|_| Errno::EIO)
    }

    fn statfs(&self) -> SysResult<Statfs> {
        let sb = &self.state.sb;
        Ok(Statfs {
            f_type: 0xEF53,
            f_bsize: sb.block_size as u64,
            f_blocks: sb.blocks_count,
            f_bfree: sb.free_blocks_count,
            f_bavail: sb.free_blocks_count,
            f_files: sb.inodes_count as u64,
            f_ffree: sb.free_inodes_count as u64,
            f_fsid: 0,
            f_namelen: 255,
            f_frsize: sb.block_size as u64,
            f_flag: if self.state.readonly { 1 } else { 0 },
            f_spare: [0; 4],
        })
    }

    fn sync(&self) -> SysResult<()> {
        self.state.driver.flush().map_err(|_| Errno::EIO)
    }
}
