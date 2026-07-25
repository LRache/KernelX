use alloc::vec;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};

use super::*;

/// Largest supported ext4 block size (64 KiB). Backs the static zero source so
/// [Context::zero_fs_block] never needs a heap allocation.
const MAX_BLOCK_SIZE: usize = 65536;

/// Zero-initialised in `.bss`; used as the source for zeroing a freshly
/// allocated block before its extent is published to disk.
static ZERO_BLOCK: [u8; MAX_BLOCK_SIZE] = [0u8; MAX_BLOCK_SIZE];

impl Context {
    pub(super) fn group_desc_location(&self, group: u32) -> SysResult<(u64, usize)> {
        if group >= self.groups_count {
            return ret_errno("group_desc_location: group out of range", Errno::EINVAL);
        }

        let desc_size = self.desc_size as usize;
        let block_size = self.block_size as usize;
        if desc_size == 0 || block_size < desc_size {
            return ret_errno("group_desc_location: invalid desc_size or block_size", Errno::EINVAL);
        }

        let descs_per_block = block_size / desc_size;
        if descs_per_block == 0 {
            return ret_errno("group_desc_location: descs_per_block is zero", Errno::EINVAL);
        }

        let group_usize = usize::try_from(group)
            .map_err(|_| debug_errno("group_desc_location: group does not fit usize", Errno::EINVAL))?;
        let table_block_index = group_usize / descs_per_block;
        let first_gdt_block = self.first_data_block as u64 + 1;
        let pblk =
            first_gdt_block
                .checked_add(u64::try_from(table_block_index).map_err(|_| {
                    debug_errno("group_desc_location: table_block_index does not fit u64", Errno::EINVAL)
                })?)
                .ok_or_else(|| debug_errno("group_desc_location: pblk overflow", Errno::EINVAL))?;

        let off = (group_usize % descs_per_block)
            .checked_mul(desc_size)
            .ok_or_else(|| debug_errno("group_desc_location: descriptor offset overflow", Errno::EINVAL))?;

        Ok((pblk, off))
    }

    pub(super) fn inode_location(&self, ino: u32) -> SysResult<(u64, usize)> {
        if ino == 0 {
            return ret_errno("inode_location: inode number is zero", Errno::EINVAL);
        }

        let idx = ino - 1;
        let group = idx / self.inodes_per_group;
        let index = idx % self.inodes_per_group;
        let gd = self.read_group_desc(group)?;

        let byte_off_in_group = (index as u64)
            .checked_mul(self.inode_size as u64)
            .ok_or_else(|| debug_errno("inode_location: byte offset overflow", Errno::EINVAL))?;

        let block_off = byte_off_in_group / self.block_size as u64;
        let off_in_block = usize::try_from(byte_off_in_group % self.block_size as u64)
            .map_err(|_| debug_errno("inode_location: in-block offset does not fit usize", Errno::EINVAL))?;

        let table_block = gd
            .inode_table_first_block
            .checked_add(block_off)
            .ok_or_else(|| debug_errno("inode_location: table block overflow", Errno::EINVAL))?;

        Ok((table_block, off_in_block))
    }

    pub(crate) fn read_fs_block(&self, pblk: u64) -> SysResult<Vec<u8>> {
        if pblk >= self.blocks_count {
            return ret_errno("read_fs_block: physical block is out of range", Errno::EIO);
        }

        let mut buf = vec![0u8; self.block_size as usize];
        self.read_fs_blocks_into(pblk, &mut buf)?;
        Ok(buf)
    }

    pub fn write_fs_block(&self, pblk: u64, buf: &[u8]) -> SysResult<()> {
        self.write_fs_blocks(pblk, buf)
    }

    /// Writes a block's worth of zeros to `pblk`. Used to initialise a freshly
    /// allocated metadata block before the extent mapping that references it is
    /// published, so a crash between publish and initialisation never exposes
    /// stale data from a previously freed block.
    pub fn zero_fs_block(&self, pblk: u64) -> SysResult<()> {
        let block_size = self.block_size as usize;
        if block_size > MAX_BLOCK_SIZE {
            return ret_errno("zero_fs_block: block size exceeds zero buffer", Errno::EINVAL);
        }
        self.write_fs_block(pblk, &ZERO_BLOCK[..block_size])
    }

    pub fn read_fs_blocks_into(&self, start_pblk: u64, dst: &mut [u8]) -> SysResult<()> {
        let block_size = self.block_size as usize;
        if dst.is_empty() {
            return Ok(());
        }
        if !dst.len().is_multiple_of(block_size) {
            return ret_errno(
                "read_fs_blocks_into: destination length is not a multiple of block size",
                Errno::EINVAL,
            );
        }
        let block_count = (dst.len() / block_size) as u64;
        let end_pblk = start_pblk
            .checked_add(block_count)
            .ok_or_else(|| debug_errno("read_fs_blocks_into: block range overflow", Errno::EINVAL))?;
        if end_pblk > self.blocks_count {
            return ret_errno("read_fs_blocks_into: physical block range is out of range", Errno::EIO);
        }

        let off = start_pblk
            .checked_mul(self.block_size as u64)
            .ok_or_else(|| debug_errno("read_fs_blocks_into: byte offset overflow", Errno::EINVAL))?;
        self.read_device(off, dst)
    }

    pub(crate) fn write_fs_blocks(&self, start_pblk: u64, buf: &[u8]) -> SysResult<()> {
        let block_size = self.block_size as usize;
        if buf.is_empty() {
            return Ok(());
        }
        if !buf.len().is_multiple_of(block_size) {
            return ret_errno(
                "write_fs_blocks: buffer length is not a multiple of block size",
                Errno::EINVAL,
            );
        }
        let block_count = (buf.len() / block_size) as u64;
        let end_pblk = start_pblk
            .checked_add(block_count)
            .ok_or_else(|| debug_errno("write_fs_blocks: block range overflow", Errno::EINVAL))?;
        if end_pblk > self.blocks_count {
            return ret_errno("write_fs_blocks: physical block range is out of range", Errno::EIO);
        }

        let off = start_pblk
            .checked_mul(self.block_size as u64)
            .ok_or_else(|| debug_errno("write_fs_blocks: byte offset overflow", Errno::EINVAL))?;
        self.write_device(off, buf)
    }

    pub(super) fn read_device(&self, offset: u64, dst: &mut [u8]) -> SysResult<()> {
        let off = usize::try_from(offset)
            .map_err(|_| debug_errno("read_device: offset does not fit usize", Errno::EINVAL))?;
        self.driver
            .read_at(off, dst)
            .map_err(|_| debug_errno("read_device: block driver read_at failed", Errno::EIO))
    }

    pub(super) fn write_device(&self, offset: u64, src: &[u8]) -> SysResult<()> {
        let off = usize::try_from(offset)
            .map_err(|_| debug_errno("write_device: offset does not fit usize", Errno::EINVAL))?;
        self.driver
            .write_at(off, src)
            .map_err(|_| debug_errno("write_device: block driver write_at failed", Errno::EIO))
    }
}
