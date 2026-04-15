use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::klib::crc::crc32c_update;

use super::*;

impl Context {
    pub fn parse_extent_root(&self, inode: &Ext4Inode) -> SysResult<ExtentBlock> {
        parse_extent_root_bytes(inode.i_block(), self.block_size as usize)
    }

    pub fn read_extent_block(&self, owner_ino: u32, owner_generation: u32, pblk: u64) -> SysResult<ExtentBlock> {
        let raw = self.read_fs_block(pblk)?;
        let block = parse_extent_block_bytes(raw, pblk, false, self.block_size as usize)?;

        if self.metadata_csum {
            self.verify_extent_block_checksum(owner_ino, owner_generation, &block)?;
        }

        Ok(block)
    }

    pub fn write_extent_block(&self, owner_ino: u32, owner_generation: u32, block: &mut ExtentBlock) -> SysResult<()> {
        if block.raw.len() != self.block_size as usize {
            return ret_errno(
                "write_extent_block: block length does not match block_size",
                Errno::EINVAL,
            );
        }

        encode_extent_block(block)?;

        if self.metadata_csum {
            self.set_extent_block_checksum(owner_ino, owner_generation, block)?;
        }

        self.write_fs_block(block.pblk, &block.raw)
    }

    fn verify_extent_block_checksum(
        &self,
        owner_ino: u32,
        owner_generation: u32,
        block: &ExtentBlock,
    ) -> SysResult<()> {
        let stored = block
            .tail_checksum
            .ok_or_else(|| debug_errno("verify_extent_block_checksum: missing tail checksum", Errno::EIO))?;
        let calc = self.extent_block_checksum(owner_ino, owner_generation, &block.raw, block.header.eh_max)?;
        if stored != calc {
            return ret_errno("verify_extent_block_checksum: checksum mismatch", Errno::EIO);
        }
        Ok(())
    }

    fn set_extent_block_checksum(
        &self,
        owner_ino: u32,
        owner_generation: u32,
        block: &mut ExtentBlock,
    ) -> SysResult<()> {
        let tail_off = extent_tail_offset(block.header.eh_max)?;
        if tail_off + 4 > block.raw.len() {
            return ret_errno(
                "set_extent_block_checksum: tail checksum field exceeds block boundary",
                Errno::EIO,
            );
        }

        let checksum = self.extent_block_checksum(owner_ino, owner_generation, &block.raw, block.header.eh_max)?;
        put_u32_le(&mut block.raw, tail_off, checksum)?;
        block.tail_checksum = Some(checksum);
        Ok(())
    }

    fn extent_block_checksum(&self, owner_ino: u32, owner_generation: u32, raw: &[u8], eh_max: u16) -> SysResult<u32> {
        let tail_off = extent_tail_offset(eh_max)?;
        if tail_off > raw.len() {
            return ret_errno("extent_block_checksum: tail offset exceeds block boundary", Errno::EIO);
        }

        let mut crc = self.checksum_seed;
        crc = crc32c_update(crc, &owner_ino.to_le_bytes());
        crc = crc32c_update(crc, &owner_generation.to_le_bytes());
        Ok(crc32c_update(crc, &raw[..tail_off]))
    }
}

fn parse_extent_root_bytes(raw: &[u8], block_size: usize) -> SysResult<ExtentBlock> {
    let (header, idx, leaf, tail_checksum) = parse_extent_block_view(raw, true, block_size)?;
    Ok(ExtentBlock {
        pblk: 0,
        raw: Vec::new(),
        header,
        idx,
        leaf,
        tail_checksum,
    })
}

fn parse_extent_block_bytes(raw: Vec<u8>, pblk: u64, is_root: bool, block_size: usize) -> SysResult<ExtentBlock> {
    let (header, idx, leaf, tail_checksum) = parse_extent_block_view(&raw, is_root, block_size)?;
    Ok(ExtentBlock {
        pblk,
        raw,
        header,
        idx,
        leaf,
        tail_checksum,
    })
}

fn parse_extent_block_view(
    raw: &[u8],
    is_root: bool,
    block_size: usize,
) -> SysResult<(ExtentHeader, Vec<ExtentIdx>, Vec<ExtentLeaf>, Option<u32>)> {
    if !is_root && raw.len() != block_size {
        return ret_errno("parse_extent_block_bytes: extent block size mismatch", Errno::EINVAL);
    }
    if raw.len() < EXTENT_HEADER_SIZE {
        return ret_errno("parse_extent_block_bytes: block smaller than extent header", Errno::EIO);
    }

    let header = parse_extent_header(raw)?;
    if header.eh_magic != EXT4_EXTENT_MAGIC {
        return ret_errno("parse_extent_block_bytes: invalid extent header magic", Errno::EIO);
    }
    if header.eh_entries > header.eh_max {
        return ret_errno("parse_extent_block_bytes: eh_entries greater than eh_max", Errno::EIO);
    }

    let entries_len = (header.eh_entries as usize)
        .checked_mul(EXTENT_ENTRY_SIZE)
        .ok_or_else(|| debug_errno("parse_extent_block_bytes: entries length overflow", Errno::EINVAL))?;
    let entries_end = EXTENT_HEADER_SIZE
        .checked_add(entries_len)
        .ok_or_else(|| debug_errno("parse_extent_block_bytes: entries end overflow", Errno::EINVAL))?;

    if entries_end > raw.len() {
        return ret_errno("parse_extent_block_bytes: entries exceed block boundary", Errno::EIO);
    }

    let mut idx = Vec::new();
    let mut leaf = Vec::new();

    if header.eh_depth == 0 {
        for i in 0..header.eh_entries as usize {
            let off = EXTENT_HEADER_SIZE + i * EXTENT_ENTRY_SIZE;
            leaf.push(ExtentLeaf {
                ee_block: get_u32_le(raw, off)?,
                ee_len_raw: get_u16_le(raw, off + 4)?,
                ee_start: ((get_u16_le(raw, off + 6)? as u64) << 32) | get_u32_le(raw, off + 8)? as u64,
            });
        }
    } else {
        for i in 0..header.eh_entries as usize {
            let off = EXTENT_HEADER_SIZE + i * EXTENT_ENTRY_SIZE;
            idx.push(ExtentIdx {
                ei_block: get_u32_le(raw, off)?,
                ei_leaf: ((get_u16_le(raw, off + 8)? as u64) << 32) | get_u32_le(raw, off + 4)? as u64,
            });
        }
    }

    let tail_checksum = if is_root {
        None
    } else {
        let tail_off = extent_tail_offset(header.eh_max)?;
        if tail_off + 4 > raw.len() {
            return ret_errno(
                "parse_extent_block_bytes: extent tail exceeds block boundary",
                Errno::EIO,
            );
        }
        Some(get_u32_le(raw, tail_off)?)
    };

    Ok((header, idx, leaf, tail_checksum))
}

fn parse_extent_header(raw: &[u8]) -> SysResult<ExtentHeader> {
    Ok(ExtentHeader {
        eh_magic: get_u16_le(raw, 0)?,
        eh_entries: get_u16_le(raw, 2)?,
        eh_max: get_u16_le(raw, 4)?,
        eh_depth: get_u16_le(raw, 6)?,
        eh_generation: get_u32_le(raw, 8)?,
    })
}

fn encode_extent_block(block: &mut ExtentBlock) -> SysResult<()> {
    if block.raw.len() < EXTENT_HEADER_SIZE {
        return ret_errno("encode_extent_block: block smaller than extent header", Errno::EINVAL);
    }

    put_u16_le(&mut block.raw, 0, block.header.eh_magic)?;
    put_u16_le(&mut block.raw, 2, block.header.eh_entries)?;
    put_u16_le(&mut block.raw, 4, block.header.eh_max)?;
    put_u16_le(&mut block.raw, 6, block.header.eh_depth)?;
    put_u32_le(&mut block.raw, 8, block.header.eh_generation)?;

    let entries = block.header.eh_entries as usize;
    let max = block.header.eh_max as usize;
    if entries > max {
        return ret_errno("encode_extent_block: eh_entries greater than eh_max", Errno::EINVAL);
    }

    if block.header.eh_depth == 0 {
        if block.leaf.len() != entries {
            return ret_errno(
                "encode_extent_block: leaf count mismatch with eh_entries",
                Errno::EINVAL,
            );
        }
        for (i, e) in block.leaf.iter().enumerate() {
            let off = EXTENT_HEADER_SIZE + i * EXTENT_ENTRY_SIZE;
            put_u32_le(&mut block.raw, off, e.ee_block)?;
            put_u16_le(&mut block.raw, off + 4, e.ee_len_raw)?;
            put_u16_le(&mut block.raw, off + 6, (e.ee_start >> 32) as u16)?;
            put_u32_le(&mut block.raw, off + 8, e.ee_start as u32)?;
        }
    } else {
        if block.idx.len() != entries {
            return ret_errno("encode_extent_block: idx count mismatch with eh_entries", Errno::EINVAL);
        }
        for (i, e) in block.idx.iter().enumerate() {
            let off = EXTENT_HEADER_SIZE + i * EXTENT_ENTRY_SIZE;
            put_u32_le(&mut block.raw, off, e.ei_block)?;
            put_u32_le(&mut block.raw, off + 4, e.ei_leaf as u32)?;
            put_u16_le(&mut block.raw, off + 8, (e.ei_leaf >> 32) as u16)?;
            put_u16_le(&mut block.raw, off + 10, 0)?;
        }
    }

    Ok(())
}

fn extent_tail_offset(eh_max: u16) -> SysResult<usize> {
    let entries_len = (eh_max as usize)
        .checked_mul(EXTENT_ENTRY_SIZE)
        .ok_or_else(|| debug_errno("extent_tail_offset: entries length overflow", Errno::EINVAL))?;
    EXTENT_HEADER_SIZE
        .checked_add(entries_len)
        .ok_or_else(|| debug_errno("extent_tail_offset: tail offset overflow", Errno::EINVAL))
}
