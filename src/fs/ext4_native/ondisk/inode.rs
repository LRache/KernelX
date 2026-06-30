use crate::kernel::errno::{Errno, SysResult};
use crate::klib::crc::crc32c_update;

use super::*;

const INODE_MODE_OFF: usize = 0x00;
const INODE_UID_OFF: usize = 0x02;
const INODE_SIZE_LO_OFF: usize = 0x04;
const INODE_GID_OFF: usize = 0x18;
const INODE_LINKS_COUNT_OFF: usize = 0x1A;
const INODE_BLOCKS_LO_OFF: usize = 0x1C;
const INODE_FLAGS_OFF: usize = 0x20;
const INODE_BLOCK_ARRAY_OFF: usize = 0x28;
pub(super) const INODE_BLOCK_ARRAY_SIZE: usize = 60;
const INODE_GENERATION_OFF: usize = 0x64;
const INODE_FILE_ACL_LO_OFF: usize = 0x68;
const INODE_SIZE_HIGH_OFF: usize = 0x6C;
const INODE_BLOCKS_HIGH_OFF: usize = 0x74;
const INODE_FILE_ACL_HIGH_OFF: usize = 0x76;
const INODE_CHECKSUM_LO_OFF: usize = 0x7C;
const INODE_EXTRA_ISIZE_OFF: usize = 0x80;
const INODE_CHECKSUM_HI_OFF: usize = 0x82;
const EXT_INIT_MAX_LEN: u16 = 32768;
const BLOCK_PTR_SIZE: usize = core::mem::size_of::<u32>();
const EXT4_INODE_DIRECT_BLOCK_COUNT: usize = 12;
const EXT4_INODE_INDIRECT_BLOCK: usize = EXT4_INODE_DIRECT_BLOCK_COUNT;
const EXT4_INODE_DOUBLE_INDIRECT_BLOCK: usize = EXT4_INODE_INDIRECT_BLOCK + 1;
const EXT4_INODE_TRIPLE_INDIRECT_BLOCK: usize = EXT4_INODE_DOUBLE_INDIRECT_BLOCK + 1;
const EXT4_INODE_BLOCKS: usize = EXT4_INODE_TRIPLE_INDIRECT_BLOCK + 1;

impl Ext4Inode {
    pub fn parse_inode(ino: u32, raw: &[u8]) -> SysResult<Self> {
        let i_size =
            ((get_u32_le(raw, INODE_SIZE_HIGH_OFF)? as u64) << 32) | get_u32_le(raw, INODE_SIZE_LO_OFF)? as u64;

        let i_blocks =
            ((get_u16_le(raw, INODE_BLOCKS_HIGH_OFF)? as u64) << 32) | get_u32_le(raw, INODE_BLOCKS_LO_OFF)? as u64;

        let i_file_acl =
            ((get_u16_le(raw, INODE_FILE_ACL_HIGH_OFF)? as u64) << 32) | get_u32_le(raw, INODE_FILE_ACL_LO_OFF)? as u64;

        Ok(Self {
            i_mode: get_u16_le(raw, INODE_MODE_OFF)?,
            i_uid: get_u16_le(raw, INODE_UID_OFF)?,
            i_gid: get_u16_le(raw, INODE_GID_OFF)?,
            i_links_count: get_u16_le(raw, INODE_LINKS_COUNT_OFF)?,
            i_size,
            i_blocks,
            i_flags: Ext4InodeFlags::from_bits_retain(get_u32_le(raw, INODE_FLAGS_OFF)?),
            i_generation: get_u32_le(raw, INODE_GENERATION_OFF)?,
            i_file_acl,
            i_extra_isize: get_u16_le(raw, INODE_EXTRA_ISIZE_OFF)?,
            raw: raw.to_vec(),
            ino,
        })
    }

    #[inline]
    pub fn i_block(&self) -> &[u8] {
        let end = INODE_BLOCK_ARRAY_OFF + INODE_BLOCK_ARRAY_SIZE;
        &self.raw[INODE_BLOCK_ARRAY_OFF..end]
    }

    #[inline]
    pub fn i_block_mut(&mut self) -> &mut [u8] {
        let end = INODE_BLOCK_ARRAY_OFF + INODE_BLOCK_ARRAY_SIZE;
        &mut self.raw[INODE_BLOCK_ARRAY_OFF..end]
    }
}

impl Context {
    pub fn read_inode(&self, ino: u32) -> SysResult<Ext4Inode> {
        let (table_block, off_in_block) = self.inode_location(ino)?;
        let block = self.read_fs_block(table_block)?;
        let inode_size = self.inode_size as usize;

        let end = off_in_block
            .checked_add(inode_size)
            .ok_or_else(|| debug_errno("read_inode: inode range overflow", Errno::EINVAL))?;
        if end > block.len() {
            return ret_errno("read_inode: inode range exceeds block boundary", Errno::EIO);
        }

        let raw = &block[off_in_block..end];
        let inode = Ext4Inode::parse_inode(ino, raw)?;

        if self.metadata_csum {
            self.verify_inode_checksum(&inode)?;
        }

        Ok(inode)
    }

    pub fn write_inode(&self, inode: &mut Ext4Inode) -> SysResult<()> {
        if inode.raw.len() != self.inode_size as usize {
            return ret_errno("write_inode: inode raw length mismatch", Errno::EINVAL);
        }

        self.encode_inode(inode)?;

        if self.metadata_csum {
            let csum = self.inode_checksum(inode.ino, inode.i_generation, &inode.raw)?;
            self.set_inode_checksum(&mut inode.raw, csum)?;
        }

        let (table_block, off_in_block) = self.inode_location(inode.ino)?;
        let mut block = self.read_fs_block(table_block)?;
        let inode_size = self.inode_size as usize;
        let end = off_in_block
            .checked_add(inode_size)
            .ok_or_else(|| debug_errno("write_inode: inode range overflow", Errno::EINVAL))?;
        if end > block.len() {
            return ret_errno("write_inode: inode range exceeds block boundary", Errno::EIO);
        }
        block[off_in_block..end].copy_from_slice(&inode.raw);
        self.write_fs_block(table_block, &block)
    }

    fn encode_inode(&self, inode: &mut Ext4Inode) -> SysResult<()> {
        if inode.raw.len() != self.inode_size as usize {
            return ret_errno("encode_inode: inode raw length mismatch", Errno::EINVAL);
        }

        put_u16_le(&mut inode.raw, INODE_MODE_OFF, inode.i_mode)?;
        put_u16_le(&mut inode.raw, INODE_UID_OFF, inode.i_uid)?;
        put_u16_le(&mut inode.raw, INODE_GID_OFF, inode.i_gid)?;
        put_u16_le(&mut inode.raw, INODE_LINKS_COUNT_OFF, inode.i_links_count)?;

        put_u32_le(&mut inode.raw, INODE_SIZE_LO_OFF, inode.i_size as u32)?;
        put_u32_le(&mut inode.raw, INODE_SIZE_HIGH_OFF, (inode.i_size >> 32) as u32)?;

        put_u32_le(&mut inode.raw, INODE_BLOCKS_LO_OFF, inode.i_blocks as u32)?;
        put_u16_le(&mut inode.raw, INODE_BLOCKS_HIGH_OFF, (inode.i_blocks >> 32) as u16)?;

        put_u32_le(&mut inode.raw, INODE_FLAGS_OFF, inode.i_flags.bits())?;
        put_u32_le(&mut inode.raw, INODE_GENERATION_OFF, inode.i_generation)?;

        put_u32_le(&mut inode.raw, INODE_FILE_ACL_LO_OFF, inode.i_file_acl as u32)?;
        put_u16_le(&mut inode.raw, INODE_FILE_ACL_HIGH_OFF, (inode.i_file_acl >> 32) as u16)?;

        put_u16_le(&mut inode.raw, INODE_EXTRA_ISIZE_OFF, inode.i_extra_isize)?;

        Ok(())
    }

    fn has_csum_hi(&self, inode_raw: &[u8]) -> SysResult<bool> {
        if self.inode_size <= 128 {
            return Ok(false);
        }
        if inode_raw.len() < INODE_EXTRA_ISIZE_OFF + 2 {
            return Ok(false);
        }
        let extra = get_u16_le(inode_raw, INODE_EXTRA_ISIZE_OFF)?;
        Ok(extra as usize >= INODE_CHECKSUM_HI_OFF + 2 - 128)
    }

    fn inode_checksum(&self, ino: u32, generation: u32, inode_raw: &[u8]) -> SysResult<u32> {
        if inode_raw.len() < INODE_CHECKSUM_LO_OFF + 2 {
            return ret_errno("inode_checksum: inode buffer too small for checksum_lo", Errno::EINVAL);
        }

        let mut crc = self.checksum_seed;
        crc = crc32c_update(crc, &ino.to_le_bytes());
        crc = crc32c_update(crc, &generation.to_le_bytes());

        crc = crc32c_update(crc, &inode_raw[..INODE_CHECKSUM_LO_OFF]);
        crc = crc32c_update(crc, &[0u8, 0u8]);

        if self.has_csum_hi(inode_raw)? {
            if inode_raw.len() < INODE_CHECKSUM_HI_OFF + 2 {
                return ret_errno("inode_checksum: inode buffer too small for checksum_hi", Errno::EINVAL);
            }
            crc = crc32c_update(crc, &inode_raw[INODE_CHECKSUM_LO_OFF + 2..INODE_CHECKSUM_HI_OFF]);
            crc = crc32c_update(crc, &[0u8, 0u8]);
            crc = crc32c_update(crc, &inode_raw[INODE_CHECKSUM_HI_OFF + 2..]);
        } else {
            crc = crc32c_update(crc, &inode_raw[INODE_CHECKSUM_LO_OFF + 2..]);
        }

        Ok(crc)
    }

    fn inode_stored_checksum(&self, inode_raw: &[u8]) -> SysResult<u32> {
        let lo = get_u16_le(inode_raw, INODE_CHECKSUM_LO_OFF)? as u32;
        if self.has_csum_hi(inode_raw)? {
            let hi = get_u16_le(inode_raw, INODE_CHECKSUM_HI_OFF)? as u32;
            Ok(lo | (hi << 16))
        } else {
            Ok(lo)
        }
    }

    fn set_inode_checksum(&self, inode_raw: &mut [u8], csum: u32) -> SysResult<()> {
        put_u16_le(inode_raw, INODE_CHECKSUM_LO_OFF, csum as u16)?;
        if self.has_csum_hi(inode_raw)? {
            put_u16_le(inode_raw, INODE_CHECKSUM_HI_OFF, (csum >> 16) as u16)?;
        }
        Ok(())
    }

    fn verify_inode_checksum(&self, inode: &Ext4Inode) -> SysResult<()> {
        let stored = self.inode_stored_checksum(&inode.raw)?;
        let calc = self.inode_checksum(inode.ino, inode.i_generation, &inode.raw)?;
        let expected = if self.has_csum_hi(&inode.raw)? {
            calc
        } else {
            calc & 0xFFFF
        };
        if stored != expected {
            return ret_errno("verify_inode_checksum: checksum mismatch", Errno::EIO);
        }
        Ok(())
    }
}

pub(crate) fn lookup_lblk(context: &Context, inode: &Ext4Inode, lblk: u32) -> SysResult<Option<u64>> {
    if inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
        let root = context.parse_extent_root(inode)?;
        return lookup_extent_lblk(context, root, inode.ino, inode.i_generation, lblk);
    }

    lookup_non_extent_lblk(context, inode, lblk)
}

pub(crate) fn lookup_extent_lblk(
    context: &Context,
    root: ExtentBlock,
    ino: u32,
    generation: u32,
    lblk: u32,
) -> SysResult<Option<u64>> {
    let mut cur = root;
    loop {
        if cur.header.eh_depth == 0 {
            return Ok(find_in_leaves(&cur.leaf, lblk));
        }

        let mut chosen_leaf: Option<u64> = None;
        for idx in &cur.idx {
            if idx.ei_block <= lblk {
                chosen_leaf = Some(idx.ei_leaf);
            } else {
                break;
            }
        }
        let next = chosen_leaf
            .or_else(|| cur.idx.first().map(|idx| idx.ei_leaf))
            .ok_or_else(|| debug_errno("lookup_lblk: extent index lookup failed", Errno::EIO))?;
        cur = context.read_extent_block(ino, generation, next)?;
    }
}

fn lookup_non_extent_lblk(context: &Context, inode: &Ext4Inode, lblk: u32) -> SysResult<Option<u64>> {
    if lblk < EXT4_INODE_DIRECT_BLOCK_COUNT as u32 {
        return read_inode_block_ptr(inode, lblk as usize).map(nonzero_block_ptr);
    }

    let blocks_id = indirect_blocks_per_level(context)?;
    let mut inode_block_limits = [0u64; 4];
    let mut inode_blocks_per_level = [0u64; 4];
    inode_block_limits[0] = EXT4_INODE_DIRECT_BLOCK_COUNT as u64;
    inode_blocks_per_level[0] = 1;

    for i in 1..4 {
        inode_blocks_per_level[i] = inode_blocks_per_level[i - 1]
            .checked_mul(blocks_id)
            .ok_or_else(|| debug_errno("lookup_lblk: inode blocks per level overflow", Errno::EINVAL))?;
        inode_block_limits[i] = inode_block_limits[i - 1]
            .checked_add(inode_blocks_per_level[i])
            .ok_or_else(|| debug_errno("lookup_lblk: inode block limit overflow", Errno::EINVAL))?;
    }

    let lblk = lblk as u64;
    let mut level = 0usize;
    for i in 1..4 {
        if lblk < inode_block_limits[i] {
            level = i;
            break;
        }
    }
    if level == 0 {
        return ret_errno(
            "lookup_lblk: logical block exceeds non-extent addressing limit",
            Errno::EIO,
        );
    }

    let mut blk_off_in_lvl = lblk
        .checked_sub(inode_block_limits[level - 1])
        .ok_or_else(|| debug_errno("lookup_lblk: logical block offset underflow", Errno::EINVAL))?;
    let mut current_block = read_inode_block_ptr(inode, EXT4_INODE_INDIRECT_BLOCK + (level - 1))?;
    let mut off_in_blk = blk_off_in_lvl / inode_blocks_per_level[level - 1];

    if current_block == 0 {
        return Ok(None);
    }

    let mut level = level;
    while level > 0 {
        current_block = read_indirect_block_ptr(context, current_block, off_in_blk)?;
        if current_block == 0 {
            return Ok(None);
        }

        level -= 1;
        if level == 0 {
            break;
        }

        blk_off_in_lvl %= inode_blocks_per_level[level];
        off_in_blk = blk_off_in_lvl / inode_blocks_per_level[level - 1];
    }

    Ok(Some(current_block as u64))
}

fn indirect_blocks_per_level(context: &Context) -> SysResult<u64> {
    let blocks_id = (context.block_size as usize) / BLOCK_PTR_SIZE;
    if blocks_id == 0 {
        return ret_errno(
            "lookup_lblk: block size is too small for indirect pointers",
            Errno::EINVAL,
        );
    }
    Ok(blocks_id as u64)
}

fn read_inode_block_ptr(inode: &Ext4Inode, index: usize) -> SysResult<u32> {
    if index >= EXT4_INODE_BLOCKS {
        return ret_errno("read_inode_block_ptr: block pointer index out of range", Errno::EINVAL);
    }
    get_u32_le(inode.i_block(), index * BLOCK_PTR_SIZE)
}

fn read_indirect_block_ptr(context: &Context, pblk: u32, index: u64) -> SysResult<u32> {
    let block = context.read_fs_block(pblk as u64)?;
    let index = usize::try_from(index).map_err(|_| {
        debug_errno(
            "read_indirect_block_ptr: indirect index does not fit usize",
            Errno::EINVAL,
        )
    })?;
    let off = index
        .checked_mul(BLOCK_PTR_SIZE)
        .ok_or_else(|| debug_errno("read_indirect_block_ptr: indirect offset overflow", Errno::EINVAL))?;
    get_u32_le(&block, off)
}

fn nonzero_block_ptr(pblk: u32) -> Option<u64> {
    if pblk == 0 { None } else { Some(pblk as u64) }
}

fn find_in_leaves(leaves: &[ExtentLeaf], lblk: u32) -> Option<u64> {
    for leaf in leaves {
        let len = extent_len(leaf.ee_len_raw);
        if lblk < leaf.ee_block {
            return None;
        }
        let end = leaf.ee_block.checked_add(len as u32)?;
        if lblk < end {
            if leaf.ee_len_raw > EXT_INIT_MAX_LEN {
                return None;
            }
            return Some(leaf.ee_start + (lblk - leaf.ee_block) as u64);
        }
    }
    None
}

fn extent_len(raw: u16) -> u16 {
    if raw <= EXT_INIT_MAX_LEN {
        raw
    } else {
        raw - EXT_INIT_MAX_LEN
    }
}

#[allow(dead_code)]
fn _reject_indexed_directory(inode: &Ext4Inode) -> SysResult<()> {
    if inode.i_flags.contains(Ext4InodeFlags::INDEX) {
        return ret_errno(
            "_reject_indexed_directory: indexed directory is not supported",
            Errno::EOPNOTSUPP,
        );
    }
    Ok(())
}
