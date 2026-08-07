use alloc::vec;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};
use crate::klib::crc::crc32c_update;

use super::*;

const DX_HASH_INFO_LEN: u8 = 8;
const DX_ROOT_INFO_OFF: usize = 24;
const DX_ENTRIES_OFF: usize = 32;
const DX_NODE_ENTRIES_OFF: usize = 8;
const DX_ENTRY_SIZE: usize = 8;

impl Context {
    pub fn read_dir_block(&self, dir_ino: u32, dir_generation: u32, pblk: u64) -> SysResult<DirBlock> {
        let raw = self.read_fs_block(pblk)?;
        let block = parse_dir_block(raw, pblk, self.metadata_csum)?;

        if self.metadata_csum && !cfg!(feature = "ext4-native-skip-crc32c-verify") {
            self.verify_dir_block_checksum(dir_ino, dir_generation, &block.raw)?;
        }

        Ok(block)
    }

    pub fn write_dir_block(&self, dir_ino: u32, dir_generation: u32, block: &mut DirBlock) -> SysResult<()> {
        let mut raw = if block.raw.len() == self.block_size as usize {
            let mut reused = core::mem::take(&mut block.raw);
            reused.fill(0);
            reused
        } else {
            vec![0u8; self.block_size as usize]
        };

        let limit = if self.metadata_csum {
            raw.len()
                .checked_sub(DIR_ENTRY_TAIL_SIZE)
                .ok_or_else(|| debug_errno("write_dir_block: block is smaller than dir tail", Errno::EINVAL))?
        } else {
            raw.len()
        };

        let mut off = 0usize;
        for entry in &mut block.entries {
            if entry.rec_len < DIR_ENTRY_HEADER_SIZE as u16 || entry.rec_len % 4 != 0 {
                return ret_errno("write_dir_block: invalid rec_len alignment or size", Errno::EINVAL);
            }

            let rec_len = entry.rec_len as usize;
            let name_len = entry.name_len as usize;
            if name_len > EXT4_MAX_NAME_LEN {
                return ret_errno("write_dir_block: file name is too long", Errno::EINVAL);
            }
            if name_len > rec_len.saturating_sub(DIR_ENTRY_HEADER_SIZE) {
                return ret_errno("write_dir_block: file name does not fit record length", Errno::EINVAL);
            }

            let end = off
                .checked_add(rec_len)
                .ok_or_else(|| debug_errno("write_dir_block: record end overflow", Errno::EINVAL))?;
            if end > limit {
                return ret_errno(
                    "write_dir_block: record exceeds writable directory region",
                    Errno::EINVAL,
                );
            }

            put_u32_le(&mut raw, off, entry.inode)?;
            put_u16_le(&mut raw, off + 4, entry.rec_len)?;
            raw[off + 6] = name_len as u8;
            raw[off + 7] = entry.file_type;
            raw[off + 8..off + 8 + name_len].copy_from_slice(entry.name_slice());

            entry.entry_off = u16::try_from(off)
                .map_err(|_| debug_errno("write_dir_block: entry offset does not fit u16", Errno::EINVAL))?;
            off = end;
        }

        if off != limit {
            return ret_errno(
                "write_dir_block: directory entries do not fully cover writable region",
                Errno::EINVAL,
            );
        }

        if self.metadata_csum {
            init_dir_entry_tail(&mut raw)?;
            self.set_dir_block_checksum(dir_ino, dir_generation, &mut raw)?;
        }

        block.raw = raw;
        self.write_fs_block(block.pblk, &block.raw)
    }

    fn dir_block_checksum(&self, dir_ino: u32, dir_generation: u32, raw: &[u8]) -> SysResult<u32> {
        let off = dir_tail_offset(raw)?
            .ok_or_else(|| debug_errno("dir_block_checksum: missing directory tail checksum entry", Errno::EIO))?;

        let mut crc = self.checksum_seed;
        crc = crc32c_update(crc, &dir_ino.to_le_bytes());
        crc = crc32c_update(crc, &dir_generation.to_le_bytes());
        Ok(crc32c_update(crc, &raw[..off]))
    }

    fn verify_dir_block_checksum(&self, dir_ino: u32, dir_generation: u32, raw: &[u8]) -> SysResult<()> {
        let Some(off) = dir_tail_offset(raw)? else {
            if try_parse_dx_node(raw, self.metadata_csum)? {
                return Ok(());
            }
            return Err(debug_errno(
                "verify_dir_block_checksum: missing directory tail checksum entry",
                Errno::EIO,
            ));
        };
        let stored = get_u32_le(raw, off + 8)?;
        let calc = self.dir_block_checksum(dir_ino, dir_generation, raw)?;
        if stored != calc {
            if is_dx_root_block(raw, self.metadata_csum)? {
                return Ok(());
            }
            return ret_errno("verify_dir_block_checksum: checksum mismatch", Errno::EOPNOTSUPP);
        }
        Ok(())
    }

    fn set_dir_block_checksum(&self, dir_ino: u32, dir_generation: u32, raw: &mut [u8]) -> SysResult<()> {
        let off = dir_tail_offset(raw)?.ok_or_else(|| {
            debug_errno(
                "set_dir_block_checksum: missing directory tail checksum entry",
                Errno::EIO,
            )
        })?;
        let calc = self.dir_block_checksum(dir_ino, dir_generation, raw)?;
        put_u32_le(raw, off + 8, calc)
    }
}

fn parse_dir_block(raw: Vec<u8>, pblk: u64, metadata_csum: bool) -> SysResult<DirBlock> {
    let block_size = raw.len();
    if block_size < DIR_ENTRY_HEADER_SIZE {
        return ret_errno(
            "parse_dir_block: block smaller than minimal dir entry header",
            Errno::EIO,
        );
    }

    if let Some(entries) = try_parse_dx_root_entries(&raw, metadata_csum)? {
        return Ok(DirBlock { pblk, raw, entries });
    }
    if try_parse_dx_node(&raw, metadata_csum)? {
        // Indexed interior nodes do not contain user-visible dirents.
        return Ok(DirBlock {
            pblk,
            raw,
            entries: Vec::new(),
        });
    }

    let tail_off = if metadata_csum { dir_tail_offset(&raw)? } else { None };

    let limit = tail_off.unwrap_or(block_size);
    let mut off = 0usize;
    let mut entries = Vec::new();

    while off < limit {
        let header_end = off
            .checked_add(DIR_ENTRY_HEADER_SIZE)
            .ok_or_else(|| debug_errno("parse_dir_block: entry header end overflow", Errno::EINVAL))?;
        if header_end > limit {
            return ret_errno("parse_dir_block: entry header exceeds directory region", Errno::EIO);
        }

        let inode = get_u32_le(&raw, off)?;
        let rec_len = get_u16_le(&raw, off + 4)?;
        let name_len = get_u8(&raw, off + 6)?;
        let file_type = get_u8(&raw, off + 7)?;

        if rec_len < DIR_ENTRY_HEADER_SIZE as u16 || rec_len % 4 != 0 {
            return ret_errno("parse_dir_block: invalid rec_len alignment or size", Errno::EIO);
        }

        let rec_len_usize = rec_len as usize;
        let rec_end = off
            .checked_add(rec_len_usize)
            .ok_or_else(|| debug_errno("parse_dir_block: record end overflow", Errno::EINVAL))?;
        if rec_end > limit {
            return ret_errno("parse_dir_block: record exceeds directory region", Errno::EIO);
        }

        if name_len as usize > rec_len_usize.saturating_sub(DIR_ENTRY_HEADER_SIZE) {
            return ret_errno("parse_dir_block: name length exceeds record payload", Errno::EIO);
        }

        let name_off = off + DIR_ENTRY_HEADER_SIZE;
        let mut name = [0u8; EXT4_MAX_NAME_LEN];
        let name_len_usize = name_len as usize;
        name[..name_len_usize].copy_from_slice(&raw[name_off..name_off + name_len_usize]);

        entries.push(DirEntry2 {
            inode,
            rec_len,
            name_len,
            file_type,
            name,
            entry_off: u16::try_from(off)
                .map_err(|_| debug_errno("parse_dir_block: entry offset does not fit u16", Errno::EINVAL))?,
        });

        off = rec_end;
    }

    if off != limit {
        return ret_errno(
            "parse_dir_block: directory parsing did not end at expected boundary",
            Errno::EIO,
        );
    }

    Ok(DirBlock { pblk, raw, entries })
}

fn try_parse_dx_root_entries(raw: &[u8], metadata_csum: bool) -> SysResult<Option<Vec<DirEntry2>>> {
    if !is_dx_root_block(raw, metadata_csum)? {
        return Ok(None);
    }

    let dot = parse_dx_fake_entry(raw, 0, b".")?
        .ok_or_else(|| debug_errno("try_parse_dx_root_entries: missing dot entry", Errno::EIO))?;
    let dotdot = parse_dx_fake_entry(raw, DIR_ENTRY_HEADER_SIZE + 4, b"..")?
        .ok_or_else(|| debug_errno("try_parse_dx_root_entries: missing dotdot entry", Errno::EIO))?;
    Ok(Some(vec![dot, dotdot]))
}

fn try_parse_dx_node(raw: &[u8], metadata_csum: bool) -> SysResult<bool> {
    if raw.len() < DX_NODE_ENTRIES_OFF + DX_ENTRY_SIZE {
        return Ok(false);
    }

    let inode = get_u32_le(raw, 0)?;
    let rec_len = get_u16_le(raw, 4)? as usize;
    let name_len = get_u8(raw, 6)?;
    let file_type = get_u8(raw, 7)?;

    if inode != 0 || name_len != 0 || file_type != 0 || rec_len != raw.len() {
        return Ok(false);
    }

    let tail_off = if metadata_csum { dir_tail_offset(raw)? } else { None };
    let limit = tail_off.unwrap_or(raw.len());
    let count = get_u16_le(raw, DX_NODE_ENTRIES_OFF + 2)? as usize;
    let dx_entries_end = DX_NODE_ENTRIES_OFF
        .checked_add(
            count
                .checked_mul(DX_ENTRY_SIZE)
                .ok_or_else(|| debug_errno("try_parse_dx_node: dx entries span overflow", Errno::EINVAL))?,
        )
        .ok_or_else(|| debug_errno("try_parse_dx_node: dx entries end overflow", Errno::EINVAL))?;

    Ok(count != 0 && dx_entries_end <= limit)
}

fn parse_dx_fake_entry(raw: &[u8], off: usize, expected_name: &[u8]) -> SysResult<Option<DirEntry2>> {
    let header_end = off
        .checked_add(DIR_ENTRY_HEADER_SIZE)
        .ok_or_else(|| debug_errno("parse_dx_fake_entry: header end overflow", Errno::EINVAL))?;
    let name_end = header_end
        .checked_add(expected_name.len())
        .ok_or_else(|| debug_errno("parse_dx_fake_entry: name end overflow", Errno::EINVAL))?;
    if name_end > raw.len() {
        return Ok(None);
    }

    let inode = get_u32_le(raw, off)?;
    let rec_len = get_u16_le(raw, off + 4)?;
    let name_len = get_u8(raw, off + 6)? as usize;
    let file_type = get_u8(raw, off + 7)?;
    if name_len != expected_name.len() || &raw[header_end..name_end] != expected_name {
        return Ok(None);
    }

    let mut name = [0u8; EXT4_MAX_NAME_LEN];
    name[..name_len].copy_from_slice(expected_name);

    Ok(Some(DirEntry2 {
        inode,
        rec_len,
        name_len: name_len as u8,
        file_type,
        name,
        entry_off: u16::try_from(off)
            .map_err(|_| debug_errno("parse_dx_fake_entry: entry offset does not fit u16", Errno::EINVAL))?,
    }))
}

fn is_dx_root_block(raw: &[u8], metadata_csum: bool) -> SysResult<bool> {
    if raw.len() < DX_ENTRIES_OFF {
        return Ok(false);
    }

    let Some(dot) = parse_dx_fake_entry(raw, 0, b".")? else {
        return Ok(false);
    };
    let Some(dotdot) = parse_dx_fake_entry(raw, DIR_ENTRY_HEADER_SIZE + 4, b"..")? else {
        return Ok(false);
    };

    if dot.rec_len as usize != DIR_ENTRY_HEADER_SIZE + 4 {
        return Ok(false);
    }
    let dotdot_end = (dotdot.entry_off as usize)
        .checked_add(dotdot.rec_len as usize)
        .ok_or_else(|| debug_errno("is_dx_root_block: dotdot end overflow", Errno::EINVAL))?;
    if dotdot_end != raw.len() {
        return Ok(false);
    }

    let reserved_zero = get_u32_le(raw, DX_ROOT_INFO_OFF)?;
    let info_length = get_u8(raw, DX_ROOT_INFO_OFF + 5)?;
    if reserved_zero != 0 || info_length != DX_HASH_INFO_LEN {
        return Ok(false);
    }

    let tail_off = if metadata_csum { dir_tail_offset(raw)? } else { None };
    let limit = tail_off.unwrap_or(raw.len());
    let count = get_u16_le(raw, DX_ENTRIES_OFF + 2)? as usize;
    let dx_entries_end = DX_ENTRIES_OFF
        .checked_add(
            count
                .checked_mul(DX_ENTRY_SIZE)
                .ok_or_else(|| debug_errno("is_dx_root_block: dx entries span overflow", Errno::EINVAL))?,
        )
        .ok_or_else(|| debug_errno("is_dx_root_block: dx entries end overflow", Errno::EINVAL))?;

    Ok(count != 0 && dx_entries_end <= limit)
}

fn dir_tail_offset(raw: &[u8]) -> SysResult<Option<usize>> {
    if raw.len() < DIR_ENTRY_TAIL_SIZE {
        return Ok(None);
    }

    let off = raw.len() - DIR_ENTRY_TAIL_SIZE;
    let reserved_zero1 = get_u32_le(raw, off)?;
    let rec_len = get_u16_le(raw, off + 4)?;
    let reserved_zero2 = get_u8(raw, off + 6)?;
    let reserved_ft = get_u8(raw, off + 7)?;

    if reserved_zero1 == 0
        && rec_len == DIR_ENTRY_TAIL_SIZE as u16
        && reserved_zero2 == 0
        && reserved_ft == EXT4_DIRENTRY_DIR_CSUM
    {
        Ok(Some(off))
    } else {
        Ok(None)
    }
}

fn init_dir_entry_tail(raw: &mut [u8]) -> SysResult<()> {
    if raw.len() < DIR_ENTRY_TAIL_SIZE {
        return ret_errno(
            "init_dir_entry_tail: block smaller than directory tail size",
            Errno::EINVAL,
        );
    }

    let off = raw.len() - DIR_ENTRY_TAIL_SIZE;
    put_u32_le(raw, off, 0)?;
    put_u16_le(raw, off + 4, DIR_ENTRY_TAIL_SIZE as u16)?;
    put_u8(raw, off + 6, 0)?;
    put_u8(raw, off + 7, EXT4_DIRENTRY_DIR_CSUM)?;
    put_u32_le(raw, off + 8, 0)?;
    Ok(())
}
