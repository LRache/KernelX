use alloc::format;
use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::Statfs;
use crate::klib::crc::crc32c_update;

use super::*;

impl Ext4Superblock {
    #[inline]
    fn inodes_count(&self) -> SysResult<u32> {
        get_u32_le(&self.raw, SB_INODES_COUNT_OFF)
    }

    fn parse_raw(raw: [u8; EXT4_SUPERBLOCK_SIZE]) -> SysResult<Self> {
        if get_u16_le(&raw, SB_MAGIC_OFF)? != EXT4_SUPER_MAGIC {
            return ret_errno("Ext4Superblock::parse_raw: invalid ext4 magic", Errno::EIO);
        }
        Ok(Self { raw })
    }

    #[inline]
    fn log_block_size(&self) -> SysResult<u32> {
        get_u32_le(&self.raw, SB_LOG_BLOCK_SIZE_OFF)
    }

    #[inline]
    fn blocks_count(&self) -> SysResult<u64> {
        let blocks_lo = get_u32_le(&self.raw, SB_BLOCKS_COUNT_LO_OFF)?;
        let blocks_hi = get_u32_le(&self.raw, SB_BLOCKS_COUNT_HI_OFF)?;
        Ok(((blocks_hi as u64) << 32) | blocks_lo as u64)
    }

    #[inline]
    pub(super) fn free_blocks_count(&self) -> SysResult<u64> {
        let blocks_lo = get_u32_le(&self.raw, SB_FREE_BLOCKS_COUNT_LO_OFF)?;
        let blocks_hi = get_u32_le(&self.raw, SB_FREE_BLOCKS_COUNT_HI_OFF)?;
        Ok(((blocks_hi as u64) << 32) | blocks_lo as u64)
    }

    #[inline]
    pub(super) fn set_free_blocks_count(&mut self, count: u64) -> SysResult<()> {
        put_u32_le(&mut self.raw, SB_FREE_BLOCKS_COUNT_LO_OFF, count as u32)?;
        put_u32_le(&mut self.raw, SB_FREE_BLOCKS_COUNT_HI_OFF, (count >> 32) as u32)
    }

    #[inline]
    pub(super) fn free_inodes_count(&self) -> SysResult<u32> {
        get_u32_le(&self.raw, SB_FREE_INODES_COUNT_OFF)
    }

    #[inline]
    pub(super) fn set_free_inodes_count(&mut self, count: u32) -> SysResult<()> {
        put_u32_le(&mut self.raw, SB_FREE_INODES_COUNT_OFF, count)
    }

    #[inline]
    fn first_data_block(&self) -> SysResult<u32> {
        get_u32_le(&self.raw, SB_FIRST_DATA_BLOCK_OFF)
    }

    #[inline]
    fn blocks_per_group(&self) -> SysResult<u32> {
        get_u32_le(&self.raw, SB_BLOCKS_PER_GROUP_OFF)
    }

    #[inline]
    fn inodes_per_group(&self) -> SysResult<u32> {
        get_u32_le(&self.raw, SB_INODES_PER_GROUP_OFF)
    }

    #[inline]
    fn inode_size(&self) -> SysResult<u16> {
        get_u16_le(&self.raw, SB_INODE_SIZE_OFF)
    }

    #[inline]
    fn desc_size(&self) -> SysResult<u16> {
        get_u16_le(&self.raw, SB_DESC_SIZE_OFF)
    }

    #[inline]
    fn feature_compat(&self) -> SysResult<u32> {
        get_u32_le(&self.raw, SB_FEATURE_COMPAT_OFF)
    }

    #[inline]
    fn feature_incompat(&self) -> SysResult<Ext4IncompatFeatures> {
        Ok(Ext4IncompatFeatures::from_bits_retain(get_u32_le(
            &self.raw,
            SB_FEATURE_INCOMPAT_OFF,
        )?))
    }

    #[inline]
    fn feature_ro_compat(&self) -> SysResult<Ext4RoCompatFeatures> {
        Ok(Ext4RoCompatFeatures::from_bits_retain(get_u32_le(
            &self.raw,
            SB_FEATURE_RO_COMPAT_OFF,
        )?))
    }

    #[inline]
    fn uuid(&self) -> SysResult<[u8; 16]> {
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(get_slice(&self.raw, SB_UUID_OFF, 16)?);
        Ok(uuid)
    }

    #[inline]
    fn checksum_type(&self) -> SysResult<u8> {
        get_u8(&self.raw, SB_CHECKSUM_TYPE_OFF)
    }

    #[inline]
    fn checksum_seed(&self) -> SysResult<u32> {
        get_u32_le(&self.raw, SB_CHECKSUM_SEED_OFF)
    }

    fn verify_checksum(&self) -> SysResult<()> {
        let stored = get_u32_le(&self.raw, SB_CHECKSUM_OFF)?;
        let calc = crc32c_update(CRC32C_INIT, &self.raw[..SB_CHECKSUM_OFF]);
        if calc != stored {
            return ret_errno("Ext4Superblock::verify_checksum: checksum mismatch", Errno::EIO);
        }
        Ok(())
    }

    fn update_checksum(&mut self) -> SysResult<()> {
        put_u32_le(&mut self.raw, SB_CHECKSUM_OFF, 0)?;
        let csum = crc32c_update(CRC32C_INIT, &self.raw[..SB_CHECKSUM_OFF]);
        put_u32_le(&mut self.raw, SB_CHECKSUM_OFF, csum)
    }
}

impl Context {
    pub fn from_device(fsno: u32, driver: Arc<dyn BlockDriverOps>) -> SysResult<Self> {
        let mut raw = [0u8; EXT4_SUPERBLOCK_SIZE];
        let off = EXT4_SUPERBLOCK_OFFSET;
        driver
            .read_at(off, &mut raw)
            .map_err(|_| debug_errno("from_device: failed to read superblock from device", Errno::EIO))?;

        let sb = Ext4Superblock::parse_raw(raw)?;
        let feature_incompat = sb.feature_incompat()?;
        let feature_ro_compat = sb.feature_ro_compat()?;
        let feature_compat = sb.feature_compat()?;

        if !SUPPORTED_INCOMPAT.contains(feature_incompat) {
            return ret_errno(
                &format!("from_device: unsupported incompat feature bits: {:?}", feature_incompat),
                Errno::EOPNOTSUPP,
            );
        }
        if !SUPPORTED_RO_COMPAT.contains(feature_ro_compat) {
            return ret_errno(
                &format!(
                    "from_device: unsupported ro_compat feature bits: {:?}",
                    feature_ro_compat
                ),
                Errno::EOPNOTSUPP,
            );
        }

        let metadata_csum = feature_ro_compat.contains(Ext4RoCompatFeatures::METADATA_CSUM);
        if metadata_csum {
            if sb.checksum_type()? != EXT4_CHECKSUM_CRC32C {
                return ret_errno(
                    "from_device: metadata_csum enabled but checksum_type is not crc32c",
                    Errno::EIO,
                );
            }
            sb.verify_checksum()?;
        }

        let log_block_size = sb.log_block_size()?;
        let sh = 10u32
            .checked_add(log_block_size)
            .ok_or_else(|| debug_errno("from_device: block size shift overflow", Errno::EINVAL))?;
        let block_size = 1u32
            .checked_shl(sh)
            .ok_or_else(|| debug_errno("from_device: invalid block size shift", Errno::EINVAL))?;

        if !(1024..=65536).contains(&block_size) || !block_size.is_power_of_two() {
            return ret_errno(
                "from_device: block size is out of supported range or not power of two",
                Errno::EINVAL,
            );
        }

        let blocks_per_group = sb.blocks_per_group()?;
        let inodes_per_group = sb.inodes_per_group()?;
        if blocks_per_group == 0 || inodes_per_group == 0 {
            return ret_errno(
                "from_device: blocks_per_group or inodes_per_group is zero",
                Errno::EINVAL,
            );
        }

        let inode_size = sb.inode_size()?;
        if inode_size < 128 || inode_size > block_size as u16 {
            return ret_errno("from_device: inode_size is invalid for block size", Errno::EINVAL);
        }

        let sb_desc_size = sb.desc_size()?;
        let desc_size = if sb_desc_size < 32 { 32 } else { sb_desc_size };
        if !(32..=64).contains(&desc_size) || (desc_size % 4) != 0 {
            return ret_errno(
                "from_device: desc_size must be in [32,64] and aligned to 4",
                Errno::EINVAL,
            );
        }

        let blocks_count = sb.blocks_count()?;
        if blocks_count == 0 {
            return ret_errno("from_device: blocks_count is zero", Errno::EINVAL);
        }

        let groups_count_u64 = blocks_count
            .checked_add(blocks_per_group as u64 - 1)
            .ok_or_else(|| debug_errno("from_device: groups_count rounding overflow", Errno::EINVAL))?
            / blocks_per_group as u64;
        let groups_count = u32::try_from(groups_count_u64)
            .map_err(|_| debug_errno("from_device: groups_count does not fit u32", Errno::EINVAL))?;

        let uuid = sb.uuid()?;
        let checksum_seed = if feature_incompat.contains(Ext4IncompatFeatures::CSUM_SEED) {
            sb.checksum_seed()?
        } else {
            crc32c_update(CRC32C_INIT, &uuid)
        };

        Ok(Self {
            fsno,
            driver,
            uuid,
            checksum_seed,
            metadata_csum,
            block_size,
            blocks_count,
            first_data_block: sb.first_data_block()?,
            blocks_per_group,
            inodes_per_group,
            inode_size,
            desc_size,
            groups_count,
            feature_compat,
            feature_incompat,
            feature_ro_compat,
        })
    }

    #[inline]
    pub fn read_superblock(&self) -> SysResult<Ext4Superblock> {
        let mut raw = [0u8; EXT4_SUPERBLOCK_SIZE];
        self.read_device(EXT4_SUPERBLOCK_OFFSET as u64, &mut raw)?;
        let sb = Ext4Superblock::parse_raw(raw)?;

        if self.metadata_csum {
            sb.verify_checksum()?;
        }

        Ok(sb)
    }

    #[inline]
    pub fn write_superblock(&self, sb: &mut Ext4Superblock) -> SysResult<()> {
        if self.metadata_csum {
            sb.update_checksum()?;
        }

        self.write_device(EXT4_SUPERBLOCK_OFFSET as u64, &sb.raw)
    }

    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    pub fn statfs(&self) -> SysResult<Statfs> {
        let sb = self.read_superblock()?;
        Ok(Statfs {
            f_type: EXT4_SUPER_MAGIC as u64,
            f_bsize: self.block_size as u64,
            f_blocks: self.blocks_count,
            f_bfree: sb.free_blocks_count()?,
            f_bavail: sb.free_blocks_count()?,
            f_files: sb.inodes_count()? as u64,
            f_ffree: sb.free_inodes_count()? as u64,
            f_fsid: self.fsno as u64,
            f_namelen: EXT4_MAX_NAME_LEN as u64,
            f_frsize: self.block_size as u64,
            f_flag: 0,
            f_spare: [0; 4],
        })
    }
}

// #[allow(dead_code)]
fn _supports_sparse_super(ctx: &Context, group: u32) -> bool {
    if !ctx.feature_ro_compat.contains(Ext4RoCompatFeatures::SPARSE_SUPER) {
        return true;
    }

    if group <= 1 {
        return true;
    }
    if group % 2 == 0 {
        return false;
    }

    is_power_of(group, 3) || is_power_of(group, 5) || is_power_of(group, 7)
}

// #[allow(dead_code)]
fn _first_meta_bg(sb_raw: &[u8; EXT4_SUPERBLOCK_SIZE]) -> SysResult<u32> {
    get_u32_le(sb_raw, SB_FIRST_META_BG_OFF)
}

fn is_power_of(mut n: u32, base: u32) -> bool {
    while n >= base {
        if n == base {
            return true;
        }
        if n % base != 0 {
            return false;
        }
        n /= base;
    }
    false
}
