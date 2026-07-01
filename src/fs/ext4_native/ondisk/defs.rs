use alloc::vec::Vec;
use bitflags::bitflags;

pub(super) const EXT4_SUPERBLOCK_OFFSET: usize = 1024;
pub(super) const EXT4_SUPERBLOCK_SIZE: usize = 1024;
pub(super) const EXT4_MAX_DESC_SIZE: usize = 64;
pub(super) const EXT4_MAX_NAME_LEN: usize = u8::MAX as usize;

pub(super) const EXT4_SUPER_MAGIC: u16 = 0xEF53;
pub(super) const EXT4_CHECKSUM_CRC32C: u8 = 1;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Ext4RoCompatFeatures: u32 {
        const SPARSE_SUPER = 0x0001;
        const LARGE_FILE = 0x0002;
        const HUGE_FILE = 0x0008;
        const GDT_CSUM = 0x0010;
        const DIR_NLINK = 0x0020;
        const EXTRA_ISIZE = 0x0040;
        const METADATA_CSUM = 0x0400;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Ext4IncompatFeatures: u32 {
        const FILETYPE = 0x0002;
        const EXTENTS = 0x0040;
        const BIT64 = 0x0080;
        const FLEX_BG = 0x0200;
        const CSUM_SEED = 0x2000;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Ext4InodeFlags: u32 {
        const INDEX = 0x0000_1000;
        const EXTENTS = 0x0008_0000;
        const INLINE_DATA = 0x1000_0000;
    }
}
pub(super) const EXT4_EXTENT_MAGIC: u16 = 0xF30A;
pub(super) const EXT4_DIRENTRY_DIR_CSUM: u8 = 0xDE;

pub(super) const SB_MAGIC_OFF: usize = 0x38;
pub(super) const SB_INODES_COUNT_OFF: usize = 0x00;
pub(super) const SB_LOG_BLOCK_SIZE_OFF: usize = 0x18;
pub(super) const SB_BLOCKS_COUNT_LO_OFF: usize = 0x04;
pub(super) const SB_BLOCKS_COUNT_HI_OFF: usize = 0x150;
pub(super) const SB_FREE_BLOCKS_COUNT_LO_OFF: usize = 0x0C;
pub(super) const SB_FREE_INODES_COUNT_OFF: usize = 0x10;
pub(super) const SB_FREE_BLOCKS_COUNT_HI_OFF: usize = 0x158;
pub(super) const SB_FIRST_DATA_BLOCK_OFF: usize = 0x14;
pub(super) const SB_BLOCKS_PER_GROUP_OFF: usize = 0x20;
pub(super) const SB_INODES_PER_GROUP_OFF: usize = 0x28;
pub(super) const SB_INODE_SIZE_OFF: usize = 0x58;
pub(super) const SB_DESC_SIZE_OFF: usize = 0xFE;
pub(super) const SB_FEATURE_COMPAT_OFF: usize = 0x5C;
pub(super) const SB_FEATURE_INCOMPAT_OFF: usize = 0x60;
pub(super) const SB_FEATURE_RO_COMPAT_OFF: usize = 0x64;
pub(super) const SB_UUID_OFF: usize = 0x68;
pub(super) const SB_CHECKSUM_TYPE_OFF: usize = 0x175;
pub(super) const SB_CHECKSUM_SEED_OFF: usize = 0x270;
pub(super) const SB_CHECKSUM_OFF: usize = 0x3FC;

pub(super) const SB_FIRST_META_BG_OFF: usize = 0x104;

pub(super) const GD_BLOCK_BITMAP_LO_OFF: usize = 0x00;
pub(super) const GD_INODE_BITMAP_LO_OFF: usize = 0x04;
pub(super) const GD_INODE_TABLE_LO_OFF: usize = 0x08;
pub(super) const GD_FREE_BLOCKS_LO_OFF: usize = 0x0C;
pub(super) const GD_FREE_INODES_LO_OFF: usize = 0x0E;
pub(super) const GD_USED_DIRS_LO_OFF: usize = 0x10;
pub(super) const GD_FLAGS_OFF: usize = 0x12;
pub(super) const GD_BLOCK_BITMAP_CSUM_LO_OFF: usize = 0x18;
pub(super) const GD_INODE_BITMAP_CSUM_LO_OFF: usize = 0x1A;
pub(super) const GD_ITABLE_UNUSED_LO_OFF: usize = 0x1C;
pub(super) const GD_CHECKSUM_OFF: usize = 0x1E;
pub(super) const GD_BLOCK_BITMAP_HI_OFF: usize = 0x20;
pub(super) const GD_INODE_BITMAP_HI_OFF: usize = 0x24;
pub(super) const GD_INODE_TABLE_HI_OFF: usize = 0x28;
pub(super) const GD_FREE_BLOCKS_HI_OFF: usize = 0x2C;
pub(super) const GD_FREE_INODES_HI_OFF: usize = 0x2E;
pub(super) const GD_USED_DIRS_HI_OFF: usize = 0x30;
pub(super) const GD_ITABLE_UNUSED_HI_OFF: usize = 0x32;
pub(super) const GD_BLOCK_BITMAP_CSUM_HI_OFF: usize = 0x38;
pub(super) const GD_INODE_BITMAP_CSUM_HI_OFF: usize = 0x3A;

pub(super) const EXT4_BG_INODE_UNINIT: u16 = 0x0001;
pub(super) const EXT4_BG_BLOCK_UNINIT: u16 = 0x0002;

pub(super) const EXTENT_HEADER_SIZE: usize = 12;
pub(super) const EXTENT_ENTRY_SIZE: usize = 12;

pub(super) const DIR_ENTRY_HEADER_SIZE: usize = 8;
pub(super) const DIR_ENTRY_TAIL_SIZE: usize = 12;

pub(super) const CRC32C_INIT: u32 = 0xFFFF_FFFF;

pub(super) const SUPPORTED_INCOMPAT: Ext4IncompatFeatures = Ext4IncompatFeatures::from_bits_retain(
    Ext4IncompatFeatures::FILETYPE.bits()
        | Ext4IncompatFeatures::EXTENTS.bits()
        | Ext4IncompatFeatures::BIT64.bits()
        | Ext4IncompatFeatures::FLEX_BG.bits()
        | Ext4IncompatFeatures::CSUM_SEED.bits(),
);

pub(super) const SUPPORTED_RO_COMPAT: Ext4RoCompatFeatures = Ext4RoCompatFeatures::from_bits_retain(
    Ext4RoCompatFeatures::SPARSE_SUPER.bits()
        | Ext4RoCompatFeatures::LARGE_FILE.bits()
        | Ext4RoCompatFeatures::HUGE_FILE.bits()
        | Ext4RoCompatFeatures::GDT_CSUM.bits()
        | Ext4RoCompatFeatures::DIR_NLINK.bits()
        | Ext4RoCompatFeatures::EXTRA_ISIZE.bits()
        | Ext4RoCompatFeatures::METADATA_CSUM.bits(),
);

#[derive(Clone)]
pub struct Ext4Superblock {
    pub raw: [u8; EXT4_SUPERBLOCK_SIZE],
}

#[derive(Clone)]
pub struct Ext4GroupDesc {
    pub raw: [u8; EXT4_MAX_DESC_SIZE],
    pub raw_len: u8,

    pub block_bitmap: u64,
    pub inode_bitmap: u64,
    pub inode_table_first_block: u64,

    pub free_blocks_count: u32,
    pub free_inodes_count: u32,
    pub used_dirs_count: u32,
    pub flags: u16,
    pub itable_unused: u32,

    pub block_bitmap_csum: u32,
    pub inode_bitmap_csum: u32,
    pub checksum: u16,
}

#[derive(Clone)]
pub struct Ext4BitmapBlock {
    pub pblk: u64,
    pub raw: Vec<u8>,
}

#[derive(Clone)]
pub struct Ext4Inode {
    pub raw: Vec<u8>,
    pub ino: u32,

    pub i_mode: u16,
    pub i_uid: u16,
    pub i_gid: u16,
    pub i_links_count: u16,

    pub i_atime: u32,
    pub i_ctime: u32,
    pub i_mtime: u32,
    pub i_size: u64,
    pub i_blocks: u64,
    pub i_flags: Ext4InodeFlags,
    pub i_generation: u32,

    pub i_file_acl: u64,
    pub i_extra_isize: u16,
}

#[derive(Clone, Copy)]
pub struct ExtentHeader {
    pub eh_magic: u16,
    pub eh_entries: u16,
    pub eh_max: u16,
    pub eh_depth: u16,
    pub eh_generation: u32,
}

#[derive(Clone, Copy)]
pub struct ExtentIdx {
    pub ei_block: u32,
    pub ei_leaf: u64,
}

#[derive(Clone, Copy)]
pub struct ExtentLeaf {
    pub ee_block: u32,
    pub ee_len_raw: u16,
    pub ee_start: u64,
}

#[derive(Clone)]
pub struct ExtentBlock {
    pub pblk: u64,
    pub raw: Vec<u8>,
    pub header: ExtentHeader,
    pub idx: Vec<ExtentIdx>,
    pub leaf: Vec<ExtentLeaf>,
    pub tail_checksum: Option<u32>,
}

#[derive(Clone)]
pub struct DirEntry2 {
    pub inode: u32,
    pub rec_len: u16,
    pub name_len: u8,
    pub file_type: u8,
    pub name: [u8; EXT4_MAX_NAME_LEN],
    pub entry_off: u16,
}

#[derive(Clone)]
pub struct DirBlock {
    pub pblk: u64,
    pub raw: Vec<u8>,
    pub entries: Vec<DirEntry2>,
}

impl Ext4GroupDesc {
    #[inline]
    pub fn raw_slice(&self) -> &[u8] {
        &self.raw[..self.raw_len as usize]
    }

    #[inline]
    pub fn raw_slice_mut(&mut self) -> &mut [u8] {
        &mut self.raw[..self.raw_len as usize]
    }
}

impl DirEntry2 {
    #[inline]
    pub fn name_slice(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}
