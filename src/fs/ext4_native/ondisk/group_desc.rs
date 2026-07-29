use crate::kernel::errno::{Errno, SysResult};
use crate::klib::crc::{crc16_update, crc32c_update};

use super::*;

impl Context {
    pub fn read_group_desc(&self, group: u32) -> SysResult<Ext4GroupDesc> {
        let (gdt_pblk, off) = self.group_desc_location(group)?;
        let block = self.read_fs_block(gdt_pblk)?;
        let desc_size = self.desc_size as usize;
        let end = off
            .checked_add(desc_size)
            .ok_or_else(|| debug_errno("read_group_desc: descriptor range overflow", Errno::EINVAL))?;
        if end > block.len() {
            return ret_errno("read_group_desc: descriptor range exceeds block boundary", Errno::EIO);
        }

        let raw = &block[off..end];
        let gd = self.parse_group_desc(group, raw)?;
        Ok(gd)
    }

    pub fn write_group_desc(&self, group: u32, gd: &mut Ext4GroupDesc) -> SysResult<()> {
        self.encode_group_desc(gd)?;

        let checksum = self.compute_group_desc_checksum(group, gd.raw_slice())?;
        put_u16_le(gd.raw_slice_mut(), GD_CHECKSUM_OFF, checksum)?;
        gd.checksum = checksum;

        let (gdt_pblk, off) = self.group_desc_location(group)?;
        let mut block = self.read_fs_block(gdt_pblk)?;
        let desc_size = self.desc_size as usize;
        let end = off
            .checked_add(desc_size)
            .ok_or_else(|| debug_errno("write_group_desc: descriptor range overflow", Errno::EINVAL))?;
        if end > block.len() {
            return ret_errno("write_group_desc: descriptor range exceeds block boundary", Errno::EIO);
        }

        block[off..end].copy_from_slice(gd.raw_slice());
        self.write_fs_block(gdt_pblk, &block)
    }

    pub(super) fn read_block_bitmap_with_group_desc(
        &self,
        group: u32,
        gd: &Ext4GroupDesc,
    ) -> SysResult<Ext4BitmapBlock> {
        let raw = self.read_fs_block(gd.block_bitmap)?;

        if self.metadata_csum && (gd.flags & EXT4_BG_BLOCK_UNINIT) == 0 {
            self.verify_bitmap_checksum(group, gd, &raw, true)?;
        }

        Ok(Ext4BitmapBlock {
            pblk: gd.block_bitmap,
            raw,
        })
    }

    pub fn write_block_bitmap(&self, group: u32, gd: &mut Ext4GroupDesc, bm: &Ext4BitmapBlock) -> SysResult<()> {
        if bm.raw.len() != self.block_size as usize {
            return ret_errno(
                "write_block_bitmap: bitmap length does not match block size",
                Errno::EINVAL,
            );
        }
        gd.flags &= !EXT4_BG_BLOCK_UNINIT;
        self.write_fs_block(bm.pblk, &bm.raw)?;

        if self.metadata_csum {
            let csum = self.bitmap_checksum(&bm.raw, true)?;
            gd.block_bitmap_csum = csum;
            self.set_group_block_bitmap_checksum(gd, csum)?;
            self.write_group_desc(group, gd)?;
        }

        Ok(())
    }

    pub fn read_inode_bitmap(&self, group: u32) -> SysResult<Ext4BitmapBlock> {
        let gd = self.read_group_desc(group)?;
        let raw = self.read_fs_block(gd.inode_bitmap)?;

        if self.metadata_csum && (gd.flags & EXT4_BG_INODE_UNINIT) == 0 {
            self.verify_bitmap_checksum(group, &gd, &raw, false)?;
        }

        Ok(Ext4BitmapBlock {
            pblk: gd.inode_bitmap,
            raw,
        })
    }

    pub fn write_inode_bitmap(&self, group: u32, gd: &mut Ext4GroupDesc, bm: &Ext4BitmapBlock) -> SysResult<()> {
        if bm.raw.len() != self.block_size as usize {
            return ret_errno(
                "write_inode_bitmap: bitmap length does not match block size",
                Errno::EINVAL,
            );
        }
        self.write_fs_block(bm.pblk, &bm.raw)?;

        if self.metadata_csum {
            let csum = self.bitmap_checksum(&bm.raw, false)?;
            gd.inode_bitmap_csum = csum;
            self.set_group_inode_bitmap_checksum(gd, csum)?;
            self.write_group_desc(group, gd)?;
        }

        Ok(())
    }

    fn parse_group_desc(&self, group: u32, raw: &[u8]) -> SysResult<Ext4GroupDesc> {
        if raw.len() != self.desc_size as usize {
            return ret_errno("parse_group_desc: raw descriptor length mismatch", Errno::EINVAL);
        }

        let block_bitmap_lo = get_u32_le(raw, GD_BLOCK_BITMAP_LO_OFF)? as u64;
        let inode_bitmap_lo = get_u32_le(raw, GD_INODE_BITMAP_LO_OFF)? as u64;
        let inode_table_lo = get_u32_le(raw, GD_INODE_TABLE_LO_OFF)? as u64;

        let (block_bitmap_hi, inode_bitmap_hi, inode_table_hi) = if self.desc_size > 32 {
            (
                get_u32_le(raw, GD_BLOCK_BITMAP_HI_OFF)? as u64,
                get_u32_le(raw, GD_INODE_BITMAP_HI_OFF)? as u64,
                get_u32_le(raw, GD_INODE_TABLE_HI_OFF)? as u64,
            )
        } else {
            (0, 0, 0)
        };

        let block_bitmap = block_bitmap_lo | (block_bitmap_hi << 32);
        let inode_bitmap = inode_bitmap_lo | (inode_bitmap_hi << 32);
        let inode_table_first_block = inode_table_lo | (inode_table_hi << 32);

        let free_blocks_lo = get_u16_le(raw, GD_FREE_BLOCKS_LO_OFF)? as u32;
        let free_inodes_lo = get_u16_le(raw, GD_FREE_INODES_LO_OFF)? as u32;
        let used_dirs_lo = get_u16_le(raw, GD_USED_DIRS_LO_OFF)? as u32;
        let itable_unused_lo = get_u16_le(raw, GD_ITABLE_UNUSED_LO_OFF)? as u32;

        let (free_blocks_hi, free_inodes_hi, used_dirs_hi, itable_unused_hi) = if self.desc_size > 32 {
            (
                get_u16_le(raw, GD_FREE_BLOCKS_HI_OFF)? as u32,
                get_u16_le(raw, GD_FREE_INODES_HI_OFF)? as u32,
                get_u16_le(raw, GD_USED_DIRS_HI_OFF)? as u32,
                get_u16_le(raw, GD_ITABLE_UNUSED_HI_OFF)? as u32,
            )
        } else {
            (0, 0, 0, 0)
        };

        let block_bitmap_csum_lo = get_u16_le(raw, GD_BLOCK_BITMAP_CSUM_LO_OFF)? as u32;
        let inode_bitmap_csum_lo = get_u16_le(raw, GD_INODE_BITMAP_CSUM_LO_OFF)? as u32;

        let (block_bitmap_csum_hi, inode_bitmap_csum_hi) = if self.desc_size >= 64 {
            (
                get_u16_le(raw, GD_BLOCK_BITMAP_CSUM_HI_OFF)? as u32,
                get_u16_le(raw, GD_INODE_BITMAP_CSUM_HI_OFF)? as u32,
            )
        } else {
            (0, 0)
        };

        let mut gd_raw = [0u8; EXT4_MAX_DESC_SIZE];
        gd_raw[..raw.len()].copy_from_slice(raw);
        let gd = Ext4GroupDesc {
            block_bitmap,
            inode_bitmap,
            inode_table_first_block,
            free_blocks_count: free_blocks_lo | (free_blocks_hi << 16),
            free_inodes_count: free_inodes_lo | (free_inodes_hi << 16),
            used_dirs_count: used_dirs_lo | (used_dirs_hi << 16),
            flags: get_u16_le(raw, GD_FLAGS_OFF)?,
            itable_unused: itable_unused_lo | (itable_unused_hi << 16),
            block_bitmap_csum: block_bitmap_csum_lo | (block_bitmap_csum_hi << 16),
            inode_bitmap_csum: inode_bitmap_csum_lo | (inode_bitmap_csum_hi << 16),
            checksum: get_u16_le(raw, GD_CHECKSUM_OFF)?,
            raw: gd_raw,
            raw_len: self.desc_size as u8,
        };

        self.verify_group_desc_checksum(group, gd.raw_slice())?;
        Ok(gd)
    }

    fn encode_group_desc(&self, gd: &mut Ext4GroupDesc) -> SysResult<()> {
        if gd.raw_len as usize != self.desc_size as usize {
            return ret_errno("encode_group_desc: raw descriptor length mismatch", Errno::EINVAL);
        }

        let block_bitmap = gd.block_bitmap;
        let inode_bitmap = gd.inode_bitmap;
        let inode_table_first_block = gd.inode_table_first_block;
        let free_blocks_count = gd.free_blocks_count;
        let free_inodes_count = gd.free_inodes_count;
        let used_dirs_count = gd.used_dirs_count;
        let flags = gd.flags;
        let itable_unused = gd.itable_unused;
        let raw = gd.raw_slice_mut();

        put_u32_le(raw, GD_BLOCK_BITMAP_LO_OFF, block_bitmap as u32)?;
        put_u32_le(raw, GD_INODE_BITMAP_LO_OFF, inode_bitmap as u32)?;
        put_u32_le(raw, GD_INODE_TABLE_LO_OFF, inode_table_first_block as u32)?;
        if self.desc_size > 32 {
            put_u32_le(raw, GD_BLOCK_BITMAP_HI_OFF, (block_bitmap >> 32) as u32)?;
            put_u32_le(raw, GD_INODE_BITMAP_HI_OFF, (inode_bitmap >> 32) as u32)?;
            put_u32_le(raw, GD_INODE_TABLE_HI_OFF, (inode_table_first_block >> 32) as u32)?;
        }

        put_u16_le(raw, GD_FREE_BLOCKS_LO_OFF, free_blocks_count as u16)?;
        put_u16_le(raw, GD_FREE_INODES_LO_OFF, free_inodes_count as u16)?;
        put_u16_le(raw, GD_USED_DIRS_LO_OFF, used_dirs_count as u16)?;
        put_u16_le(raw, GD_FLAGS_OFF, flags)?;
        put_u16_le(raw, GD_ITABLE_UNUSED_LO_OFF, itable_unused as u16)?;

        if self.desc_size > 32 {
            put_u16_le(raw, GD_FREE_BLOCKS_HI_OFF, (free_blocks_count >> 16) as u16)?;
            put_u16_le(raw, GD_FREE_INODES_HI_OFF, (free_inodes_count >> 16) as u16)?;
            put_u16_le(raw, GD_USED_DIRS_HI_OFF, (used_dirs_count >> 16) as u16)?;
            put_u16_le(raw, GD_ITABLE_UNUSED_HI_OFF, (itable_unused >> 16) as u16)?;
        }

        self.set_group_block_bitmap_checksum(gd, gd.block_bitmap_csum)?;
        self.set_group_inode_bitmap_checksum(gd, gd.inode_bitmap_csum)?;

        Ok(())
    }

    fn set_group_block_bitmap_checksum(&self, gd: &mut Ext4GroupDesc, csum: u32) -> SysResult<()> {
        put_u16_le(gd.raw_slice_mut(), GD_BLOCK_BITMAP_CSUM_LO_OFF, csum as u16)?;
        if self.desc_size >= 64 {
            put_u16_le(gd.raw_slice_mut(), GD_BLOCK_BITMAP_CSUM_HI_OFF, (csum >> 16) as u16)?;
        }
        gd.block_bitmap_csum = csum;
        Ok(())
    }

    fn set_group_inode_bitmap_checksum(&self, gd: &mut Ext4GroupDesc, csum: u32) -> SysResult<()> {
        put_u16_le(gd.raw_slice_mut(), GD_INODE_BITMAP_CSUM_LO_OFF, csum as u16)?;
        if self.desc_size >= 64 {
            put_u16_le(gd.raw_slice_mut(), GD_INODE_BITMAP_CSUM_HI_OFF, (csum >> 16) as u16)?;
        }
        gd.inode_bitmap_csum = csum;
        Ok(())
    }

    fn compute_group_desc_checksum(&self, group: u32, raw: &[u8]) -> SysResult<u16> {
        if raw.len() != self.desc_size as usize {
            return ret_errno(
                "compute_group_desc_checksum: raw descriptor length mismatch",
                Errno::EINVAL,
            );
        }

        if self.metadata_csum {
            let group_le = group.to_le_bytes();

            let mut crc = self.checksum_seed;
            crc = crc32c_update(crc, &group_le);
            crc = crc32c_update(crc, &raw[..GD_CHECKSUM_OFF]);
            crc = crc32c_update(crc, &[0u8, 0u8]);
            if raw.len() > GD_CHECKSUM_OFF + 2 {
                crc = crc32c_update(crc, &raw[GD_CHECKSUM_OFF + 2..]);
            }
            return Ok((crc & 0xFFFF) as u16);
        }

        if self.feature_ro_compat.contains(Ext4RoCompatFeatures::GDT_CSUM) {
            let mut crc = crc16_update(0xFFFF, &self.uuid);
            let group_le = group.to_le_bytes();
            crc = crc16_update(crc, &group_le);

            crc = crc16_update(crc, &raw[..GD_CHECKSUM_OFF]);
            if raw.len() > GD_CHECKSUM_OFF + 2 && self.feature_incompat.contains(Ext4IncompatFeatures::BIT64) {
                crc = crc16_update(crc, &raw[GD_CHECKSUM_OFF + 2..]);
            }
            return Ok(crc);
        }

        Ok(0)
    }

    fn verify_group_desc_checksum(&self, group: u32, raw: &[u8]) -> SysResult<()> {
        if !self.metadata_csum && !self.feature_ro_compat.contains(Ext4RoCompatFeatures::GDT_CSUM) {
            return Ok(());
        }

        let stored = get_u16_le(raw, GD_CHECKSUM_OFF)?;
        let calc = self.compute_group_desc_checksum(group, raw)?;
        if stored != calc {
            return ret_errno("verify_group_desc_checksum: checksum mismatch", Errno::EIO);
        }
        Ok(())
    }

    fn bitmap_checksum(&self, bitmap: &[u8], is_block_bitmap: bool) -> SysResult<u32> {
        let bits = if is_block_bitmap {
            self.blocks_per_group
        } else {
            self.inodes_per_group
        };
        let checksum_len = (bits as usize)
            .checked_add(7)
            .ok_or_else(|| debug_errno("bitmap_checksum: checksum length overflow", Errno::EINVAL))?
            / 8;
        if checksum_len > bitmap.len() {
            return ret_errno("bitmap_checksum: bitmap shorter than checksum length", Errno::EIO);
        }

        Ok(crc32c_update(self.checksum_seed, &bitmap[..checksum_len]))
    }

    fn verify_bitmap_checksum(
        &self,
        group: u32,
        gd: &Ext4GroupDesc,
        bitmap: &[u8],
        is_block_bitmap: bool,
    ) -> SysResult<()> {
        let stored = if is_block_bitmap {
            gd.block_bitmap_csum
        } else {
            gd.inode_bitmap_csum
        };

        let checksum_len = if is_block_bitmap {
            (self.blocks_per_group as usize)
                .checked_add(7)
                .ok_or_else(|| debug_errno("verify_bitmap_checksum: checksum length overflow", Errno::EINVAL))?
                / 8
        } else {
            (self.inodes_per_group as usize)
                .checked_add(7)
                .ok_or_else(|| debug_errno("verify_bitmap_checksum: checksum length overflow", Errno::EINVAL))?
                / 8
        };
        let calc = self.bitmap_checksum(bitmap, is_block_bitmap)?;

        if self.desc_size < 64 {
            if (stored & 0xFFFF) != (calc & 0xFFFF) {
                crate::kinfo!(
                    "[ext4_native] bitmap checksum mismatch kind={} group={} pblk={} desc_size={} seed={:#x} stored={:#x} calc={:#x} compare_bits=16 checksum_len={}",
                    if is_block_bitmap { "block" } else { "inode" },
                    group,
                    if is_block_bitmap {
                        gd.block_bitmap
                    } else {
                        gd.inode_bitmap
                    },
                    self.desc_size,
                    self.checksum_seed,
                    stored,
                    calc,
                    checksum_len
                );
                return ret_errno("verify_bitmap_checksum: low-16 checksum mismatch", Errno::EIO);
            }
        } else if stored != calc {
            crate::kinfo!(
                "[ext4_native] bitmap checksum mismatch kind={} group={} pblk={} desc_size={} seed={:#x} stored={:#x} calc={:#x} compare_bits=32 checksum_len={}",
                if is_block_bitmap { "block" } else { "inode" },
                group,
                if is_block_bitmap {
                    gd.block_bitmap
                } else {
                    gd.inode_bitmap
                },
                self.desc_size,
                self.checksum_seed,
                stored,
                calc,
                checksum_len
            );
            return ret_errno("verify_bitmap_checksum: checksum mismatch", Errno::EIO);
        }

        Ok(())
    }
}

pub fn test_bit(bitmap: &Ext4BitmapBlock, idx: u32) -> bool {
    let byte = (idx / 8) as usize;
    if byte >= bitmap.raw.len() {
        return false;
    }
    let bit = idx % 8;
    (bitmap.raw[byte] & (1u8 << bit)) != 0
}

pub fn set_bit(bitmap: &mut Ext4BitmapBlock, idx: u32) {
    let byte = (idx / 8) as usize;
    if byte >= bitmap.raw.len() {
        return;
    }
    let bit = idx % 8;
    bitmap.raw[byte] |= 1u8 << bit;
}

pub fn clear_bit(bitmap: &mut Ext4BitmapBlock, idx: u32) {
    let byte = (idx / 8) as usize;
    if byte >= bitmap.raw.len() {
        return;
    }
    let bit = idx % 8;
    bitmap.raw[byte] &= !(1u8 << bit);
}
