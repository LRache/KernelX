use bitflags::bitflags;

use crate::driver::BlockDriverOps;
use crate::kernel::errno::{Errno, SysResult};

const EXT4_SUPERBLOCK_OFFSET: usize = 1024;
const EXT4_SUPERBLOCK_SIZE: usize = 1024;
const EXT4_SUPERBLOCK_MAGIC: u16 = 0xEF53;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct CompatFeatures: u32 {
        const HAS_JOURNAL = 0x0004;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct IncompatFeatures: u32 {
        const FILETYPE = 0x0002;
        const RECOVER = 0x0004;
        const META_BG = 0x0010;
        const EXTENTS = 0x0040;
        const BIT64 = 0x0080;
        const FLEX_BG = 0x0200;
        const INLINE_DATA = 0x8000;
        const ENCRYPT = 0x10000;
        const CASEFOLD = 0x40000;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct RoCompatFeatures: u32 {
        const SPARSE_SUPER = 0x0001;
        const LARGE_FILE = 0x0002;
        const HUGE_FILE = 0x0008;
        const GDT_CSUM = 0x0010;
        const DIR_NLINK = 0x0020;
        const EXTRA_ISIZE = 0x0040;
        const METADATA_CSUM = 0x0400;
        const PROJECT = 0x2000;
        const VERITY = 0x8000;
        const ORPHAN_PRESENT = 0x10000;
    }
}

const SUPPORTED_INCOMPAT: IncompatFeatures = IncompatFeatures::FILETYPE
    .union(IncompatFeatures::RECOVER)
    .union(IncompatFeatures::EXTENTS)
    .union(IncompatFeatures::BIT64)
    .union(IncompatFeatures::FLEX_BG);
const SUPPORTED_RO_COMPAT: RoCompatFeatures = RoCompatFeatures::SPARSE_SUPER
    .union(RoCompatFeatures::LARGE_FILE)
    .union(RoCompatFeatures::HUGE_FILE)
    .union(RoCompatFeatures::GDT_CSUM)
    .union(RoCompatFeatures::DIR_NLINK)
    .union(RoCompatFeatures::EXTRA_ISIZE)
    .union(RoCompatFeatures::METADATA_CSUM);

#[derive(Debug, Clone, Copy)]
pub struct Ext4FeatureSet {
    pub compat: CompatFeatures,
    pub incompat: IncompatFeatures,
    pub ro_compat: RoCompatFeatures,
    pub has_journal: bool,
    pub needs_recovery: bool,
    pub requires_readonly: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Ext4SuperBlockDisk {
    pub inodes_count: u32,
    pub blocks_count: u64,
    pub free_blocks_count: u64,
    pub free_inodes_count: u32,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub block_size: u32,
    pub inode_size: u16,
    pub desc_size: u16,
    pub first_data_block: u32,
    pub features: Ext4FeatureSet,
}

fn le_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn le_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn combine_u64(lo: u32, hi: u32) -> u64 {
    (lo as u64) | ((hi as u64) << 32)
}

pub fn load_superblock(driver: &dyn BlockDriverOps) -> SysResult<Ext4SuperBlockDisk> {
    let mut raw = [0u8; EXT4_SUPERBLOCK_SIZE];
    driver
        .read_at(EXT4_SUPERBLOCK_OFFSET, &mut raw)
        .map_err(|_| Errno::EIO)?;

    let magic = le_u16(&raw, 0x38);
    if magic != EXT4_SUPERBLOCK_MAGIC {
        return Err(Errno::EINVAL);
    }

    let incompat_raw = le_u32(&raw, 0x60);
    let ro_compat_raw = le_u32(&raw, 0x64);
    let compat_raw = le_u32(&raw, 0x5C);

    let incompat = IncompatFeatures::from_bits_retain(incompat_raw);
    let ro_compat = RoCompatFeatures::from_bits_retain(ro_compat_raw);
    let compat = CompatFeatures::from_bits_retain(compat_raw);

    let has_64bit = incompat.contains(IncompatFeatures::BIT64);
    let has_journal = compat.contains(CompatFeatures::HAS_JOURNAL);
    let needs_recovery = incompat.contains(IncompatFeatures::RECOVER);
    let requires_readonly = (ro_compat_raw & !SUPPORTED_RO_COMPAT.bits()) != 0;

    let blocks_count_lo = le_u32(&raw, 0x04);
    let free_blocks_lo = le_u32(&raw, 0x0C);
    let blocks_count_hi = if has_64bit { le_u32(&raw, 0x150) } else { 0 };
    let free_blocks_hi = if has_64bit { le_u32(&raw, 0x158) } else { 0 };
    let log_block_size = le_u32(&raw, 0x18);
    if log_block_size > 6 {
        return Err(Errno::EINVAL);
    }
    let block_size = 1024u32.checked_shl(log_block_size).ok_or(Errno::EINVAL)?;
    if !(1024..=65536).contains(&block_size) {
        return Err(Errno::EINVAL);
    }

    let desc_size_raw = le_u16(&raw, 0xFE);
    let desc_size = if has_64bit {
        core::cmp::max(32, desc_size_raw)
    } else {
        32
    };

    let features = Ext4FeatureSet {
        compat,
        incompat,
        ro_compat,
        has_journal,
        needs_recovery,
        requires_readonly,
    };

    Ok(Ext4SuperBlockDisk {
        inodes_count: le_u32(&raw, 0x00),
        blocks_count: combine_u64(blocks_count_lo, blocks_count_hi),
        free_blocks_count: combine_u64(free_blocks_lo, free_blocks_hi),
        free_inodes_count: le_u32(&raw, 0x10),
        blocks_per_group: le_u32(&raw, 0x20),
        inodes_per_group: le_u32(&raw, 0x28),
        block_size,
        inode_size: le_u16(&raw, 0x58),
        desc_size,
        first_data_block: le_u32(&raw, 0x14),
        features,
    })
}

pub fn validate_feature_set(fs: &Ext4FeatureSet) -> SysResult<()> {
    if !fs.incompat.difference(SUPPORTED_INCOMPAT).is_empty() {
        return Err(Errno::EOPNOTSUPP);
    }

    if fs
        .incompat
        .intersects(IncompatFeatures::INLINE_DATA | IncompatFeatures::ENCRYPT | IncompatFeatures::CASEFOLD)
    {
        return Err(Errno::EOPNOTSUPP);
    }

    if fs
        .ro_compat
        .intersects(RoCompatFeatures::PROJECT | RoCompatFeatures::VERITY | RoCompatFeatures::ORPHAN_PRESENT)
    {
        return Err(Errno::EOPNOTSUPP);
    }

    Ok(())
}
