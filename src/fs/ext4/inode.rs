use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::cmp::min;
use core::mem;
use core::time::Duration;

use crate::driver::chosen::kclock;
use crate::fs::ext4::ondisk::IncompatFeatures;
use crate::fs::ext4::superblock::Ext4RuntimeState;
use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::fs::inode::{InodeOps, Mode, Owner};
use crate::fs::{Dentry, FileType};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{FileStat, Uid};
use crate::klib::SpinLock;

const EXT4_EXT_MAGIC: u16 = 0xF30A;
const EXT4_SUPERBLOCK_OFFSET: usize = 1024;
const SB_FREE_BLOCKS_LO_OFF: usize = EXT4_SUPERBLOCK_OFFSET + 0x0C;
const SB_FREE_BLOCKS_HI_OFF: usize = EXT4_SUPERBLOCK_OFFSET + 0x158;
const SB_FREE_INODES_LO_OFF: usize = EXT4_SUPERBLOCK_OFFSET + 0x10;
// Note: s_free_inodes_count is a single u32 — there is no high part.

bitflags! {
    #[derive(Clone, Copy)]
    struct InodeFlags: u32 {
        const EXTENTS = 0x0008_0000;
    }
}

#[derive(Clone)]
struct Ext4InodeDisk {
    mode: u16,
    uid: u32,
    gid: u32,
    size: u64,
    nlink: u16,
    atime: u32,
    ctime: u32,
    mtime: u32,
    blocks_512: u64,
    flags: InodeFlags,
    block: [u8; 60],
}

#[derive(Clone, Copy)]
struct ExtentEntry {
    lblk: u32,
    len: u16,
    pblk: u64,
}

struct DirtyWrite {
    offset: u64,
    data: Vec<u8>,
}

struct DirEntryRaw {
    ino: u32,
    rec_len: usize,
    name: String,
    file_type: u8,
    offset: usize,
}

pub struct Ext4Inode {
    ino: u32,
    state: Arc<Ext4RuntimeState>,
    write_cache: SpinLock<Vec<DirtyWrite>>,
}

impl Ext4Inode {
    pub fn new(ino: u32, state: Arc<Ext4RuntimeState>) -> Self {
        Self {
            ino,
            state,
            write_cache: SpinLock::new(Vec::new(), "Ext4Inode::write_cache"),
        }
    }

    fn inode_disk(&self) -> SysResult<Ext4InodeDisk> {
        self.state.read_inode(self.ino)
    }

    fn flush_cached_writes(&self) -> SysResult<()> {
        let pending = {
            let mut cache = self.write_cache.lock();
            mem::take(&mut *cache)
        };
        if pending.is_empty() {
            return Ok(());
        }

        let mut inode = self.inode_disk()?;
        let mut iter = pending.into_iter();
        while let Some(write) = iter.next() {
            if let Err(e) = self
                .state
                .write_file_at(self.ino, &mut inode, &write.data, write.offset)
            {
                let mut cache = self.write_cache.lock();
                cache.push(write);
                cache.extend(iter);
                return Err(e);
            }
        }
        Ok(())
    }

    fn overlay_cached_writes(&self, buf: &mut [u8], offset: u64) {
        let end = offset.saturating_add(buf.len() as u64);
        let cache = self.write_cache.lock();
        for w in cache.iter() {
            let w_start = w.offset;
            let w_end = w.offset.saturating_add(w.data.len() as u64);
            let ov_start = core::cmp::max(offset, w_start);
            let ov_end = core::cmp::min(end, w_end);
            if ov_start >= ov_end {
                continue;
            }
            let dst_start = (ov_start - offset) as usize;
            let src_start = (ov_start - w_start) as usize;
            let len = (ov_end - ov_start) as usize;
            buf[dst_start..dst_start + len].copy_from_slice(&w.data[src_start..src_start + len]);
        }
    }

    fn with_mut_inode(&self, f: impl FnOnce(&mut Ext4InodeDisk) -> SysResult<()>) -> SysResult<()> {
        if self.state.readonly {
            return Err(Errno::EROFS);
        }
        let mut inode = self.inode_disk()?;
        f(&mut inode)?;
        self.state.write_inode_back(self.ino, &inode)
    }

    fn populate_new_entry(
        &self,
        parent: &mut Ext4InodeDisk,
        name: &str,
        mode: Mode,
        new_ino: u32,
        new_inode: &mut Ext4InodeDisk,
    ) -> SysResult<()> {
        let ft = mode_to_dirent_type(mode.bits() as u16);
        if mode.contains(Mode::S_IFDIR) {
            self.state.init_new_directory(new_ino, new_inode, self.ino)?;
            parent.nlink = parent.nlink.saturating_add(1);
            self.state.write_inode_back(self.ino, parent)?;
        }
        self.state.add_dir_entry(self.ino, parent, name, new_ino, ft)?;
        self.state.write_inode_back(self.ino, parent)
    }
}

fn le_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn le_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn mode_to_file_type(mode: u16) -> FileType {
    match mode & 0o170000 {
        0o100000 => FileType::Regular,
        0o040000 => FileType::Directory,
        0o120000 => FileType::Symlink,
        0o020000 => FileType::CharDevice,
        0o060000 => FileType::BlockDevice,
        0o010000 => FileType::FIFO,
        0o140000 => FileType::Socket,
        _ => FileType::Unknown,
    }
}

fn dirent_type_to_file_type(ft: u8) -> FileType {
    match ft {
        1 => FileType::Regular,
        2 => FileType::Directory,
        3 => FileType::CharDevice,
        4 => FileType::BlockDevice,
        5 => FileType::FIFO,
        6 => FileType::Socket,
        7 => FileType::Symlink,
        _ => FileType::Unknown,
    }
}

fn mode_to_dirent_type(mode: u16) -> u8 {
    match mode_to_file_type(mode) {
        FileType::Regular => 1,
        FileType::Directory => 2,
        FileType::CharDevice => 3,
        FileType::BlockDevice => 4,
        FileType::FIFO => 5,
        FileType::Socket => 6,
        FileType::Symlink => 7,
        FileType::Unknown => 0,
    }
}

fn dirent_need_len(name_len: usize) -> usize {
    (8 + name_len + 3) & !3
}

impl Ext4RuntimeState {
    fn inode_disk_location(&self, ino: u32) -> SysResult<(usize, usize)> {
        if ino == 0 || ino > self.sb.inodes_count {
            return Err(Errno::ENOENT);
        }
        let inodes_per_group = self.sb.inodes_per_group;
        if inodes_per_group == 0 {
            return Err(Errno::EINVAL);
        }
        let inode_size = self.sb.inode_size as usize;
        if inode_size < 128 {
            return Err(Errno::EINVAL);
        }
        let idx = (ino - 1) as usize;
        let group = (idx as u32) / inodes_per_group;
        let local = (idx as u32) % inodes_per_group;
        let inode_table_block = self.read_group_desc_inode_table_block(group)?;
        let bs = self.block_size_usize()?;
        let table_byte = (inode_table_block as usize)
            .checked_mul(bs)
            .and_then(|v| v.checked_add((local as usize).checked_mul(inode_size)?))
            .ok_or(Errno::EINVAL)?;
        Ok((table_byte, inode_size))
    }

    fn read_inode(&self, ino: u32) -> SysResult<Ext4InodeDisk> {
        let (table_byte, inode_size) = self.inode_disk_location(ino)?;

        let mut raw = vec![0u8; inode_size];
        self.read_bytes(table_byte, &mut raw)?;

        let mode = le_u16(&raw, 0x00);
        let uid_lo = le_u16(&raw, 0x02) as u32;
        let gid_lo = le_u16(&raw, 0x18) as u32;
        let uid_hi = if inode_size >= 128 {
            le_u16(&raw, 0x78) as u32
        } else {
            0
        };
        let gid_hi = if inode_size >= 128 {
            le_u16(&raw, 0x7A) as u32
        } else {
            0
        };

        let size_lo = le_u32(&raw, 0x04) as u64;
        let size_hi = le_u32(&raw, 0x6C) as u64;
        let blocks_lo = le_u32(&raw, 0x1C) as u64;
        let blocks_hi = if inode_size >= 128 {
            le_u16(&raw, 0x74) as u64
        } else {
            0
        };

        let mut block = [0u8; 60];
        block.copy_from_slice(&raw[0x28..0x28 + 60]);

        Ok(Ext4InodeDisk {
            mode,
            uid: uid_lo | (uid_hi << 16),
            gid: gid_lo | (gid_hi << 16),
            size: size_lo | (size_hi << 32),
            nlink: le_u16(&raw, 0x1A),
            atime: le_u32(&raw, 0x08),
            ctime: le_u32(&raw, 0x0C),
            mtime: le_u32(&raw, 0x10),
            blocks_512: blocks_lo | (blocks_hi << 32),
            flags: InodeFlags::from_bits_retain(le_u32(&raw, 0x20)),
            block,
        })
    }

    fn write_inode_back(&self, ino: u32, inode: &Ext4InodeDisk) -> SysResult<()> {
        let (table_byte, inode_size) = self.inode_disk_location(ino)?;
        let mut raw = vec![0u8; inode_size];
        self.read_bytes(table_byte, &mut raw)?;

        raw[0x00..0x02].copy_from_slice(&inode.mode.to_le_bytes());
        raw[0x02..0x04].copy_from_slice(&(inode.uid as u16).to_le_bytes());
        raw[0x18..0x1A].copy_from_slice(&(inode.gid as u16).to_le_bytes());
        raw[0x78..0x7A].copy_from_slice(&((inode.uid >> 16) as u16).to_le_bytes());
        raw[0x7A..0x7C].copy_from_slice(&((inode.gid >> 16) as u16).to_le_bytes());
        raw[0x08..0x0C].copy_from_slice(&inode.atime.to_le_bytes());
        raw[0x0C..0x10].copy_from_slice(&inode.ctime.to_le_bytes());
        raw[0x10..0x14].copy_from_slice(&inode.mtime.to_le_bytes());
        raw[0x1A..0x1C].copy_from_slice(&inode.nlink.to_le_bytes());
        raw[0x04..0x08].copy_from_slice(&(inode.size as u32).to_le_bytes());
        raw[0x6C..0x70].copy_from_slice(&((inode.size >> 32) as u32).to_le_bytes());

        raw[0x1C..0x20].copy_from_slice(&(inode.blocks_512 as u32).to_le_bytes());
        raw[0x74..0x76].copy_from_slice(&((inode.blocks_512 >> 32) as u16).to_le_bytes());

        raw[0x20..0x24].copy_from_slice(&inode.flags.bits().to_le_bytes());
        raw[0x28..0x28 + 60].copy_from_slice(&inode.block);

        self.write_bytes(table_byte, &raw)
    }

    fn read_group_desc_raw(&self, group: u32) -> SysResult<(usize, Vec<u8>)> {
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
        Ok((desc_off, desc))
    }

    fn group_block_bitmap_block(&self, desc: &[u8]) -> u64 {
        let lo = u32::from_le_bytes([desc[0], desc[1], desc[2], desc[3]]) as u64;
        let hi = if self.sb.desc_size >= 64 {
            u32::from_le_bytes([desc[32], desc[33], desc[34], desc[35]]) as u64
        } else {
            0
        };
        lo | (hi << 32)
    }

    fn group_inode_bitmap_block(&self, desc: &[u8]) -> u64 {
        let lo = u32::from_le_bytes([desc[4], desc[5], desc[6], desc[7]]) as u64;
        let hi = if self.sb.desc_size >= 64 {
            u32::from_le_bytes([desc[36], desc[37], desc[38], desc[39]]) as u64
        } else {
            0
        };
        lo | (hi << 32)
    }

    fn dec_group_free_blocks(&self, desc: &mut [u8]) -> SysResult<()> {
        let lo = u16::from_le_bytes([desc[12], desc[13]]) as u32;
        let hi = if self.sb.desc_size >= 64 {
            u16::from_le_bytes([desc[44], desc[45]]) as u32
        } else {
            0
        };
        let val = (lo | (hi << 16)).checked_sub(1).ok_or(Errno::ENOSPC)?;
        desc[12..14].copy_from_slice(&(val as u16).to_le_bytes());
        if self.sb.desc_size >= 64 {
            desc[44..46].copy_from_slice(&((val >> 16) as u16).to_le_bytes());
        }
        Ok(())
    }

    fn dec_group_free_inodes(&self, desc: &mut [u8]) -> SysResult<()> {
        let lo = u16::from_le_bytes([desc[14], desc[15]]) as u32;
        let hi = if self.sb.desc_size >= 64 {
            u16::from_le_bytes([desc[46], desc[47]]) as u32
        } else {
            0
        };
        let val = (lo | (hi << 16)).checked_sub(1).ok_or(Errno::ENOSPC)?;
        desc[14..16].copy_from_slice(&(val as u16).to_le_bytes());
        if self.sb.desc_size >= 64 {
            desc[46..48].copy_from_slice(&((val >> 16) as u16).to_le_bytes());
        }
        Ok(())
    }

    fn dec_super_free_blocks(&self) -> SysResult<()> {
        let mut lo_buf = [0u8; 4];
        self.read_bytes(SB_FREE_BLOCKS_LO_OFF, &mut lo_buf)?;
        let lo = u32::from_le_bytes(lo_buf);

        let mut hi = 0u32;
        if self.feature_set.incompat.contains(IncompatFeatures::BIT64) {
            let mut hi_buf = [0u8; 4];
            self.read_bytes(SB_FREE_BLOCKS_HI_OFF, &mut hi_buf)?;
            hi = u32::from_le_bytes(hi_buf);
        }

        let val = ((lo as u64) | ((hi as u64) << 32))
            .checked_sub(1)
            .ok_or(Errno::ENOSPC)?;
        self.write_bytes(SB_FREE_BLOCKS_LO_OFF, &(val as u32).to_le_bytes())?;
        if self.feature_set.incompat.contains(IncompatFeatures::BIT64) {
            self.write_bytes(SB_FREE_BLOCKS_HI_OFF, &((val >> 32) as u32).to_le_bytes())?;
        }
        Ok(())
    }

    fn dec_super_free_inodes(&self) -> SysResult<()> {
        let mut buf = [0u8; 4];
        self.read_bytes(SB_FREE_INODES_LO_OFF, &mut buf)?;
        let val = u32::from_le_bytes(buf).checked_sub(1).ok_or(Errno::ENOSPC)?;
        self.write_bytes(SB_FREE_INODES_LO_OFF, &val.to_le_bytes())
    }

    fn inc_group_free_blocks(&self, desc: &mut [u8]) -> SysResult<()> {
        let lo = u16::from_le_bytes([desc[12], desc[13]]) as u32;
        let hi = if self.sb.desc_size >= 64 {
            u16::from_le_bytes([desc[44], desc[45]]) as u32
        } else {
            0
        };
        let val = (lo | (hi << 16)).saturating_add(1);
        desc[12..14].copy_from_slice(&(val as u16).to_le_bytes());
        if self.sb.desc_size >= 64 {
            desc[44..46].copy_from_slice(&((val >> 16) as u16).to_le_bytes());
        }
        Ok(())
    }

    fn inc_group_free_inodes(&self, desc: &mut [u8]) -> SysResult<()> {
        let lo = u16::from_le_bytes([desc[14], desc[15]]) as u32;
        let hi = if self.sb.desc_size >= 64 {
            u16::from_le_bytes([desc[46], desc[47]]) as u32
        } else {
            0
        };
        let val = (lo | (hi << 16)).saturating_add(1);
        desc[14..16].copy_from_slice(&(val as u16).to_le_bytes());
        if self.sb.desc_size >= 64 {
            desc[46..48].copy_from_slice(&((val >> 16) as u16).to_le_bytes());
        }
        Ok(())
    }

    fn inc_super_free_blocks(&self) -> SysResult<()> {
        let mut lo_buf = [0u8; 4];
        self.read_bytes(SB_FREE_BLOCKS_LO_OFF, &mut lo_buf)?;
        let lo = u32::from_le_bytes(lo_buf);
        let mut hi = 0u32;
        if self.feature_set.incompat.contains(IncompatFeatures::BIT64) {
            let mut hi_buf = [0u8; 4];
            self.read_bytes(SB_FREE_BLOCKS_HI_OFF, &mut hi_buf)?;
            hi = u32::from_le_bytes(hi_buf);
        }
        let val = ((lo as u64) | ((hi as u64) << 32)).saturating_add(1);
        self.write_bytes(SB_FREE_BLOCKS_LO_OFF, &(val as u32).to_le_bytes())?;
        if self.feature_set.incompat.contains(IncompatFeatures::BIT64) {
            self.write_bytes(SB_FREE_BLOCKS_HI_OFF, &((val >> 32) as u32).to_le_bytes())?;
        }
        Ok(())
    }

    fn inc_super_free_inodes(&self) -> SysResult<()> {
        let mut buf = [0u8; 4];
        self.read_bytes(SB_FREE_INODES_LO_OFF, &mut buf)?;
        let val = u32::from_le_bytes(buf).saturating_add(1);
        self.write_bytes(SB_FREE_INODES_LO_OFF, &val.to_le_bytes())
    }

    fn alloc_one_block(&self) -> SysResult<u64> {
        let groups = self.group_count();
        let bs = self.block_size_usize()?;
        for g in 0..groups {
            let (desc_off, mut desc) = self.read_group_desc_raw(g)?;
            let bitmap_blk = self.group_block_bitmap_block(&desc);
            let mut bitmap = self.read_block_alloc(bitmap_blk)?;

            for bit in 0..(bs * 8) {
                let byte = bit / 8;
                let mask = 1u8 << (bit % 8);
                if (bitmap[byte] & mask) != 0 {
                    continue;
                }

                let block_in_group = bit as u64;
                let pblk = self.sb.first_data_block as u64
                    + (g as u64)
                        .checked_mul(self.sb.blocks_per_group as u64)
                        .ok_or(Errno::EINVAL)?
                    + block_in_group;
                if pblk >= self.sb.blocks_count {
                    break;
                }

                bitmap[byte] |= mask;
                let bitmap_off = (bitmap_blk as usize).checked_mul(bs).ok_or(Errno::EINVAL)?;
                self.write_bytes(bitmap_off, &bitmap)?;
                self.dec_group_free_blocks(&mut desc)?;
                self.write_bytes(desc_off, &desc)?;
                self.dec_super_free_blocks()?;
                return Ok(pblk);
            }
        }
        Err(Errno::ENOSPC)
    }

    fn alloc_inode(&self, mode: Mode, owner: Owner) -> SysResult<(u32, Ext4InodeDisk)> {
        let groups = self.group_count();
        let now = kclock::now().unwrap_or(Duration::ZERO).as_secs().min(u32::MAX as u64) as u32;

        for g in 0..groups {
            let (desc_off, mut desc) = self.read_group_desc_raw(g)?;
            let bitmap_blk = self.group_inode_bitmap_block(&desc);
            let mut bitmap = self.read_block_alloc(bitmap_blk)?;
            let inodes_per_group = self.sb.inodes_per_group as usize;

            for bit in 0..inodes_per_group {
                let byte = bit / 8;
                let mask = 1u8 << (bit % 8);
                if (bitmap[byte] & mask) != 0 {
                    continue;
                }
                let ino = g
                    .checked_mul(self.sb.inodes_per_group)
                    .and_then(|v| v.checked_add(bit as u32 + 1))
                    .ok_or(Errno::EINVAL)?;
                if ino == 0 || ino > self.sb.inodes_count {
                    break;
                }

                bitmap[byte] |= mask;
                let bs = self.block_size_usize()?;
                let bitmap_off = (bitmap_blk as usize).checked_mul(bs).ok_or(Errno::EINVAL)?;
                self.write_bytes(bitmap_off, &bitmap)?;
                self.dec_group_free_inodes(&mut desc)?;
                self.write_bytes(desc_off, &desc)?;
                self.dec_super_free_inodes()?;

                let mut block = [0u8; 60];
                block[0..2].copy_from_slice(&EXT4_EXT_MAGIC.to_le_bytes());
                block[2..4].copy_from_slice(&0u16.to_le_bytes());
                block[4..6].copy_from_slice(&4u16.to_le_bytes());
                block[6..8].copy_from_slice(&0u16.to_le_bytes());
                block[8..10].copy_from_slice(&0u16.to_le_bytes());

                let inode = Ext4InodeDisk {
                    mode: mode.bits() as u16,
                    uid: owner.uid,
                    gid: owner.gid,
                    size: 0,
                    nlink: if mode.contains(Mode::S_IFDIR) { 2 } else { 1 },
                    atime: now,
                    ctime: now,
                    mtime: now,
                    blocks_512: 0,
                    flags: InodeFlags::EXTENTS,
                    block,
                };
                self.write_inode_back(ino, &inode)?;
                return Ok((ino, inode));
            }
        }

        Err(Errno::ENOSPC)
    }

    fn free_block(&self, pblk: u64) -> SysResult<()> {
        let first_data = self.sb.first_data_block as u64;
        if pblk < first_data || pblk >= self.sb.blocks_count {
            return Err(Errno::EINVAL);
        }
        let relative = pblk - first_data;
        let bpg = self.sb.blocks_per_group as u64;
        if bpg == 0 {
            return Err(Errno::EINVAL);
        }
        let group = (relative / bpg) as u32;
        let bit = (relative % bpg) as usize;

        let (desc_off, mut desc) = self.read_group_desc_raw(group)?;
        let bitmap_blk = self.group_block_bitmap_block(&desc);
        let bs = self.block_size_usize()?;
        let mut bitmap = self.read_block_alloc(bitmap_blk)?;

        let byte = bit / 8;
        let mask = 1u8 << (bit % 8);
        if (bitmap[byte] & mask) == 0 {
            return Ok(());
        }
        bitmap[byte] &= !mask;
        let bitmap_off = (bitmap_blk as usize).checked_mul(bs).ok_or(Errno::EINVAL)?;
        self.write_bytes(bitmap_off, &bitmap)?;
        self.inc_group_free_blocks(&mut desc)?;
        self.write_bytes(desc_off, &desc)?;
        self.inc_super_free_blocks()?;
        Ok(())
    }

    fn free_inode(&self, ino: u32) -> SysResult<()> {
        if ino == 0 || ino > self.sb.inodes_count {
            return Err(Errno::EINVAL);
        }
        let idx = (ino - 1) as usize;
        let ipg = self.sb.inodes_per_group as usize;
        if ipg == 0 {
            return Err(Errno::EINVAL);
        }
        let group = (idx / ipg) as u32;
        let bit = idx % ipg;

        let (desc_off, mut desc) = self.read_group_desc_raw(group)?;
        let bitmap_blk = self.group_inode_bitmap_block(&desc);
        let bs = self.block_size_usize()?;
        let mut bitmap = self.read_block_alloc(bitmap_blk)?;

        let byte = bit / 8;
        let mask = 1u8 << (bit % 8);
        if (bitmap[byte] & mask) == 0 {
            return Ok(());
        }
        bitmap[byte] &= !mask;
        let bitmap_off = (bitmap_blk as usize).checked_mul(bs).ok_or(Errno::EINVAL)?;
        self.write_bytes(bitmap_off, &bitmap)?;
        self.inc_group_free_inodes(&mut desc)?;
        self.write_bytes(desc_off, &desc)?;
        self.inc_super_free_inodes()?;
        Ok(())
    }

    fn free_inode_blocks(&self, inode: &Ext4InodeDisk) -> SysResult<()> {
        if !inode.flags.contains(InodeFlags::EXTENTS) {
            return Ok(());
        }
        if Self::extent_depth(&inode.block) != 0 {
            return Ok(());
        }
        let (_, _, entries) = Self::parse_extent_entries(&inode.block)?;
        for e in &entries {
            for i in 0..e.len as u64 {
                let _ = self.free_block(e.pblk + i);
            }
        }
        Ok(())
    }

    fn extent_depth(block: &[u8; 60]) -> u16 {
        le_u16(block, 6)
    }

    fn parse_extent_entries(block: &[u8; 60]) -> SysResult<(usize, u16, Vec<ExtentEntry>)> {
        let magic = le_u16(block, 0);
        if magic != EXT4_EXT_MAGIC {
            return Err(Errno::EIO);
        }
        let entries = le_u16(block, 2) as usize;
        let max_entries = le_u16(block, 4) as usize;
        let depth = le_u16(block, 6);
        if entries > max_entries || max_entries > 4 {
            return Err(Errno::EIO);
        }
        if depth != 0 {
            return Err(Errno::ENOSYS);
        }

        let mut result = Vec::with_capacity(entries);
        for i in 0..entries {
            let off = 12 + i * 12;
            let lblk = le_u32(block, off);
            let len_raw = le_u16(block, off + 4);
            let len = len_raw & 0x7FFF;
            if len == 0 {
                continue;
            }
            let start_hi = le_u16(block, off + 6) as u64;
            let start_lo = le_u32(block, off + 8) as u64;
            result.push(ExtentEntry {
                lblk,
                len,
                pblk: start_lo | (start_hi << 32),
            });
        }
        Ok((max_entries, le_u16(block, 8), result))
    }

    fn write_extent_entries(block: &mut [u8; 60], generation: u16, entries: &[ExtentEntry], max_entries: usize) {
        block.fill(0);
        block[0..2].copy_from_slice(&EXT4_EXT_MAGIC.to_le_bytes());
        block[2..4].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        block[4..6].copy_from_slice(&(max_entries as u16).to_le_bytes());
        block[6..8].copy_from_slice(&0u16.to_le_bytes());
        block[8..10].copy_from_slice(&generation.to_le_bytes());
        for (i, e) in entries.iter().enumerate() {
            let off = 12 + i * 12;
            block[off..off + 4].copy_from_slice(&e.lblk.to_le_bytes());
            block[off + 4..off + 6].copy_from_slice(&e.len.to_le_bytes());
            block[off + 6..off + 8].copy_from_slice(&((e.pblk >> 32) as u16).to_le_bytes());
            block[off + 8..off + 12].copy_from_slice(&(e.pblk as u32).to_le_bytes());
        }
    }

    fn insert_or_extend_extent(&self, inode: &mut Ext4InodeDisk, lblk: u32, pblk: u64) -> SysResult<()> {
        if !inode.flags.contains(InodeFlags::EXTENTS) {
            return Err(Errno::ENOSYS);
        }
        if Self::extent_depth(&inode.block) != 0 {
            return Err(Errno::ENOSYS);
        }

        let (max_entries, generation, mut entries) = Self::parse_extent_entries(&inode.block)?;
        for e in &entries {
            if lblk >= e.lblk && lblk < e.lblk + e.len as u32 {
                return Ok(());
            }
        }

        entries.sort_by_key(|e| e.lblk);
        for i in 0..entries.len() {
            let e = entries[i];
            let e_end_lblk = e.lblk + e.len as u32;
            let e_end_pblk = e.pblk + e.len as u64;
            if e_end_lblk == lblk && e_end_pblk == pblk && e.len < 0x7FFF {
                entries[i].len += 1;
                Self::write_extent_entries(&mut inode.block, generation, &entries, max_entries);
                return Ok(());
            }
            if lblk + 1 == e.lblk && pblk + 1 == e.pblk && e.len < 0x7FFF {
                entries[i].lblk -= 1;
                entries[i].pblk -= 1;
                entries[i].len += 1;
                Self::write_extent_entries(&mut inode.block, generation, &entries, max_entries);
                return Ok(());
            }
        }

        if entries.len() >= max_entries {
            return Err(Errno::ENOSPC);
        }
        entries.push(ExtentEntry { lblk, len: 1, pblk });
        entries.sort_by_key(|e| e.lblk);
        Self::write_extent_entries(&mut inode.block, generation, &entries, max_entries);
        Ok(())
    }

    /// Truncate extents so that only logical blocks `[0, keep_lblks)` are kept.
    /// Frees physical blocks beyond the boundary and updates the extent tree.
    fn truncate_extents(&self, inode: &mut Ext4InodeDisk, keep_lblks: u32) -> SysResult<()> {
        if !inode.flags.contains(InodeFlags::EXTENTS) {
            return Ok(());
        }
        if Self::extent_depth(&inode.block) != 0 {
            return Err(Errno::ENOSYS);
        }

        let (max_entries, generation, entries) = Self::parse_extent_entries(&inode.block)?;
        let bs = self.block_size() as u64;
        let mut new_entries = Vec::new();

        for e in &entries {
            let e_end = e.lblk + e.len as u32;
            if e.lblk >= keep_lblks {
                // Entire extent beyond boundary — free all blocks
                for i in 0..e.len as u64 {
                    let _ = self.free_block(e.pblk + i);
                }
                inode.blocks_512 = inode.blocks_512.saturating_sub(e.len as u64 * (bs / 512));
            } else if e_end > keep_lblks {
                // Partially beyond boundary — trim and free tail
                let keep = (keep_lblks - e.lblk) as u16;
                let freed = e.len - keep;
                for i in keep as u64..e.len as u64 {
                    let _ = self.free_block(e.pblk + i);
                }
                inode.blocks_512 = inode.blocks_512.saturating_sub(freed as u64 * (bs / 512));
                new_entries.push(ExtentEntry {
                    lblk: e.lblk,
                    len: keep,
                    pblk: e.pblk,
                });
            } else {
                // Entirely within boundary — keep as is
                new_entries.push(*e);
            }
        }

        Self::write_extent_entries(&mut inode.block, generation, &new_entries, max_entries);
        Ok(())
    }
}

impl Ext4RuntimeState {
    fn extent_map_lblk(&self, inode: &Ext4InodeDisk, lblk: u32) -> SysResult<Option<u64>> {
        if !inode.flags.contains(InodeFlags::EXTENTS) {
            return Err(Errno::ENOSYS);
        }
        self.extent_map_from_node(&inode.block, lblk)
    }

    fn extent_map_from_node(&self, node: &[u8], lblk: u32) -> SysResult<Option<u64>> {
        if node.len() < 12 {
            return Err(Errno::EIO);
        }
        let magic = le_u16(node, 0);
        if magic != EXT4_EXT_MAGIC {
            return Err(Errno::EIO);
        }
        let entries = le_u16(node, 2) as usize;
        let max_entries = le_u16(node, 4) as usize;
        let depth = le_u16(node, 6);
        if entries > max_entries {
            return Err(Errno::EIO);
        }
        let required = 12usize
            .checked_add(entries.checked_mul(12).ok_or(Errno::EINVAL)?)
            .ok_or(Errno::EINVAL)?;
        if node.len() < required {
            return Err(Errno::EIO);
        }

        if depth == 0 {
            for i in 0..entries {
                let off = 12 + i * 12;
                let ee_block = le_u32(node, off);
                let ee_len_raw = le_u16(node, off + 4);
                let ee_start_hi = le_u16(node, off + 6) as u64;
                let ee_start_lo = le_u32(node, off + 8) as u64;
                let ee_len = (ee_len_raw & 0x7FFF) as u32;
                if ee_len == 0 {
                    continue;
                }
                if lblk < ee_block || lblk >= ee_block + ee_len {
                    continue;
                }
                let start = ee_start_lo | (ee_start_hi << 32);
                return Ok(Some(start + (lblk - ee_block) as u64));
            }
            return Ok(None);
        }

        let mut chosen: Option<usize> = None;
        for i in 0..entries {
            let off = 12 + i * 12;
            let ei_block = le_u32(node, off);
            if ei_block <= lblk {
                chosen = Some(off);
            } else {
                break;
            }
        }
        let off = match chosen {
            Some(v) => v,
            None => return Ok(None),
        };
        let leaf_lo = le_u32(node, off + 4) as u64;
        let leaf_hi = le_u16(node, off + 8) as u64;
        let child_blk = leaf_lo | (leaf_hi << 32);
        let child = self.read_block_alloc(child_blk)?;
        self.extent_map_from_node(&child, lblk)
    }

    fn read_file_at(&self, inode: &Ext4InodeDisk, buf: &mut [u8], offset: u64) -> SysResult<usize> {
        if offset >= inode.size {
            return Ok(0);
        }
        let bs = self.block_size_usize()?;
        let mut done = 0usize;
        let mut off = offset;
        let max_len = min(buf.len() as u64, inode.size - offset) as usize;
        let mut tmp = Vec::new();
        while done < max_len {
            let lblk = (off / bs as u64) as u32;
            let in_blk_off = (off % bs as u64) as usize;
            let chunk = min(max_len - done, bs - in_blk_off);
            if let Some(pblk) = self.extent_map_lblk(inode, lblk)? {
                if in_blk_off == 0 && chunk == bs {
                    // Aligned full-block read: read directly into caller's buffer
                    self.read_block(pblk, &mut buf[done..done + bs])?;
                } else {
                    // Partial-block read: reuse tmp buffer
                    tmp.resize(bs, 0);
                    self.read_block(pblk, &mut tmp)?;
                    buf[done..done + chunk].copy_from_slice(&tmp[in_blk_off..in_blk_off + chunk]);
                }
            } else {
                buf[done..done + chunk].fill(0);
            }
            done += chunk;
            off += chunk as u64;
        }
        Ok(done)
    }

    fn write_file_at(&self, ino: u32, inode: &mut Ext4InodeDisk, buf: &[u8], offset: u64) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let end = offset.saturating_add(buf.len() as u64);
        let bs = self.block_size() as u64;
        let mut done = 0usize;
        let mut off = offset;
        let mut inode_dirty = false;
        while done < buf.len() {
            let lblk = (off / bs) as u32;
            let in_blk_off = (off % bs) as usize;
            let chunk = min(buf.len() - done, (bs as usize) - in_blk_off);
            let pblk = match self.extent_map_lblk(inode, lblk)? {
                Some(v) => v,
                None => {
                    let new_pblk = self.alloc_one_block()?;
                    if in_blk_off != 0 || chunk != bs as usize {
                        let blk_base = (new_pblk as usize).checked_mul(bs as usize).ok_or(Errno::EINVAL)?;
                        self.write_bytes(blk_base, &vec![0u8; bs as usize])?;
                    }
                    self.insert_or_extend_extent(inode, lblk, new_pblk)?;
                    inode.blocks_512 = inode.blocks_512.saturating_add(bs / 512);
                    inode_dirty = true;
                    new_pblk
                }
            };
            let block_base = (pblk as usize).checked_mul(bs as usize).ok_or(Errno::EINVAL)?;
            let byte_off = block_base.checked_add(in_blk_off).ok_or(Errno::EINVAL)?;
            self.write_bytes(byte_off, &buf[done..done + chunk])?;
            done += chunk;
            off += chunk as u64;
        }
        if end > inode.size {
            inode.size = end;
            inode_dirty = true;
        }
        if inode_dirty {
            self.write_inode_back(ino, inode)?;
        }
        Ok(done)
    }

    fn read_dirent_at(&self, inode: &Ext4InodeDisk, mut off: usize) -> SysResult<Option<(DirResult, usize)>> {
        let size = inode.size as usize;
        let bs = self.block_size_usize()?;
        let mut cached_lblk = u32::MAX;
        let mut blk_buf = Vec::new();

        while off < size {
            let lblk = (off / bs) as u32;
            let blk_off = off % bs;

            if lblk != cached_lblk {
                match self.extent_map_lblk(inode, lblk)? {
                    Some(pblk) => {
                        blk_buf.resize(bs, 0);
                        self.read_block(pblk, &mut blk_buf)?;
                    }
                    None => {
                        off = (lblk as usize + 1) * bs;
                        continue;
                    }
                }
                cached_lblk = lblk;
            }

            if blk_off + 8 > bs {
                return Err(Errno::EIO);
            }
            let child_ino = le_u32(&blk_buf, blk_off);
            let rec_len = le_u16(&blk_buf, blk_off + 4) as usize;
            let name_len = blk_buf[blk_off + 6] as usize;
            let file_type = blk_buf[blk_off + 7];
            if rec_len < 8 || (rec_len & 0x3) != 0 {
                return Err(Errno::EIO);
            }
            let next = off.checked_add(rec_len).ok_or(Errno::EINVAL)?;
            if next > size {
                return Err(Errno::EIO);
            }
            if child_ino == 0 {
                off = next;
                continue;
            }
            if name_len > rec_len - 8 || blk_off + 8 + name_len > bs {
                return Err(Errno::EIO);
            }
            let name = String::from_utf8_lossy(&blk_buf[blk_off + 8..blk_off + 8 + name_len]).into_owned();
            return Ok(Some((
                DirResult {
                    name,
                    ino: child_ino,
                    file_type: dirent_type_to_file_type(file_type),
                },
                next,
            )));
        }
        Ok(None)
    }

    fn read_dirent_raw_at(&self, inode: &Ext4InodeDisk, off: usize) -> SysResult<Option<DirEntryRaw>> {
        if off >= inode.size as usize {
            return Ok(None);
        }
        let mut hdr = [0u8; 8];
        let n = self.read_file_at(inode, &mut hdr, off as u64)?;
        if n < 8 {
            return Ok(None);
        }
        let child_ino = le_u32(&hdr, 0);
        let rec_len = le_u16(&hdr, 4) as usize;
        let name_len = hdr[6] as usize;
        let file_type = hdr[7];
        if rec_len < 8 || (rec_len & 0x3) != 0 || off + rec_len > inode.size as usize {
            return Err(Errno::EIO);
        }
        let mut name_buf = vec![0u8; name_len];
        if name_len > 0 {
            self.read_file_at(inode, &mut name_buf, (off + 8) as u64)?;
        }
        Ok(Some(DirEntryRaw {
            ino: child_ino,
            rec_len,
            name: String::from_utf8_lossy(&name_buf).into_owned(),
            file_type,
            offset: off,
        }))
    }

    fn write_dirent(
        &self,
        dir_ino: u32,
        dir_inode: &mut Ext4InodeDisk,
        off: usize,
        rec_len: usize,
        child_ino: u32,
        file_type: u8,
        name: &str,
    ) -> SysResult<()> {
        let name_bytes = name.as_bytes();
        if name_bytes.len() > 255 || rec_len < dirent_need_len(name_bytes.len()) {
            return Err(Errno::EINVAL);
        }
        let mut data = vec![0u8; rec_len];
        data[0..4].copy_from_slice(&child_ino.to_le_bytes());
        data[4..6].copy_from_slice(&(rec_len as u16).to_le_bytes());
        data[6] = name_bytes.len() as u8;
        data[7] = file_type;
        data[8..8 + name_bytes.len()].copy_from_slice(name_bytes);
        self.write_file_at(dir_ino, dir_inode, &data, off as u64)?;
        Ok(())
    }

    fn add_dir_entry(
        &self,
        dir_ino: u32,
        dir_inode: &mut Ext4InodeDisk,
        name: &str,
        child_ino: u32,
        file_type: u8,
    ) -> SysResult<()> {
        if name.is_empty() || name.len() > 255 {
            return Err(Errno::EINVAL);
        }
        if mode_to_file_type(dir_inode.mode) != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        if self.lookup_name(dir_inode, name)?.is_some() {
            return Err(Errno::EEXIST);
        }

        let need = dirent_need_len(name.len());
        let mut off = 0usize;
        while off < dir_inode.size as usize {
            let ent = match self.read_dirent_raw_at(dir_inode, off)? {
                Some(v) => v,
                None => break,
            };

            if ent.ino == 0 && ent.rec_len >= need {
                return self.write_dirent(dir_ino, dir_inode, off, ent.rec_len, child_ino, file_type, name);
            }

            let actual = dirent_need_len(ent.name.len());
            if ent.rec_len >= actual + need {
                // Write new entry first — if the second write (shrinking the old
                // entry) fails, the old entry still covers the entire region and
                // the new entry is harmlessly hidden in its padding.
                self.write_dirent(
                    dir_ino,
                    dir_inode,
                    off + actual,
                    ent.rec_len - actual,
                    child_ino,
                    file_type,
                    name,
                )?;
                return self.write_dirent(dir_ino, dir_inode, off, actual, ent.ino, ent.file_type, &ent.name);
            }

            off = ent.offset + ent.rec_len;
        }

        let bs = self.block_size() as usize;
        if bs == 0 {
            return Err(Errno::EINVAL);
        }
        let append_off = dir_inode.size as usize;
        let lblk = (append_off / bs) as u32;
        let pblk = self.alloc_one_block()?;
        self.insert_or_extend_extent(dir_inode, lblk, pblk)?;
        dir_inode.blocks_512 = dir_inode.blocks_512.saturating_add((self.block_size() as u64) / 512);
        dir_inode.size = dir_inode.size.saturating_add(bs as u64);
        self.write_inode_back(dir_ino, dir_inode)?;
        self.write_dirent(dir_ino, dir_inode, append_off, bs, child_ino, file_type, name)
    }

    fn lookup_name(&self, dir_inode: &Ext4InodeDisk, name: &str) -> SysResult<Option<(u32, usize)>> {
        let size = dir_inode.size as usize;
        let bs = self.block_size_usize()?;
        let name_bytes = name.as_bytes();
        let mut off = 0usize;
        let mut cached_lblk = u32::MAX;
        let mut blk_buf = Vec::new();

        while off < size {
            let lblk = (off / bs) as u32;
            let blk_off = off % bs;

            if lblk != cached_lblk {
                match self.extent_map_lblk(dir_inode, lblk)? {
                    Some(pblk) => {
                        blk_buf.resize(bs, 0);
                        self.read_block(pblk, &mut blk_buf)?;
                    }
                    None => {
                        off = (lblk as usize + 1) * bs;
                        continue;
                    }
                }
                cached_lblk = lblk;
            }

            if blk_off + 8 > bs {
                return Err(Errno::EIO);
            }
            let child_ino = le_u32(&blk_buf, blk_off);
            let rec_len = le_u16(&blk_buf, blk_off + 4) as usize;
            let name_len = blk_buf[blk_off + 6] as usize;
            if rec_len < 8 || (rec_len & 0x3) != 0 || off + rec_len > size {
                return Err(Errno::EIO);
            }
            if child_ino != 0 && name_len == name_bytes.len() && blk_off + 8 + name_len <= bs {
                if blk_buf[blk_off + 8..blk_off + 8 + name_len] == *name_bytes {
                    return Ok(Some((child_ino, off)));
                }
            }
            off += rec_len;
        }
        Ok(None)
    }

    fn mark_dirent_unused(&self, dir_ino: u32, dir_inode: &mut Ext4InodeDisk, off: usize) -> SysResult<()> {
        let mut ino_zero = [0u8; 4];
        ino_zero.copy_from_slice(&0u32.to_le_bytes());
        self.write_file_at(dir_ino, dir_inode, &ino_zero, off as u64)?;
        Ok(())
    }

    fn dir_is_empty(&self, dir_inode: &Ext4InodeDisk) -> SysResult<bool> {
        let mut off = 0usize;
        while off < dir_inode.size as usize {
            let ent = match self.read_dirent_raw_at(dir_inode, off)? {
                Some(v) => v,
                None => break,
            };
            if ent.ino != 0 && ent.name != "." && ent.name != ".." {
                return Ok(false);
            }
            off = ent.offset + ent.rec_len;
        }
        Ok(true)
    }

    fn init_new_directory(&self, ino: u32, dir_inode: &mut Ext4InodeDisk, parent_ino: u32) -> SysResult<()> {
        let bs = self.block_size_usize()?;
        let pblk = self.alloc_one_block()?;
        self.insert_or_extend_extent(dir_inode, 0, pblk)?;
        dir_inode.blocks_512 = dir_inode.blocks_512.saturating_add((self.block_size() as u64) / 512);
        dir_inode.size = self.block_size() as u64;

        let dot_len = dirent_need_len(1);
        self.write_dirent(ino, dir_inode, 0, dot_len, ino, 2, ".")?;
        self.write_dirent(ino, dir_inode, dot_len, bs - dot_len, parent_ino, 2, "..")?;
        self.write_inode_back(ino, dir_inode)?;
        Ok(())
    }
}

impl InodeOps for Ext4Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "ext4-native"
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        if self.state.readonly {
            return Err(Errno::EROFS);
        }
        let mut parent = self.inode_disk()?;
        if mode_to_file_type(parent.mode) != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        if self.state.lookup_name(&parent, name)?.is_some() {
            return Err(Errno::EEXIST);
        }

        let (new_ino, mut new_inode) = self.state.alloc_inode(mode, owner)?;

        if let Err(e) = self.populate_new_entry(&mut parent, name, mode, new_ino, &mut new_inode) {
            let _ = self.state.free_inode_blocks(&new_inode);
            let _ = self.state.free_inode(new_ino);
            return Err(e);
        }

        Ok(Arc::new(Self::new(new_ino, self.state.clone())))
    }

    fn link(&self, name: &str, target: &Arc<dyn InodeOps>) -> SysResult<()> {
        if self.state.readonly {
            return Err(Errno::EROFS);
        }
        let target_ino = target.get_ino();
        let mut target_inode = self.state.read_inode(target_ino)?;
        if mode_to_file_type(target_inode.mode) == FileType::Directory {
            return Err(Errno::EISDIR);
        }

        let mut parent = self.inode_disk()?;
        if mode_to_file_type(parent.mode) != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }

        let ft = mode_to_dirent_type(target_inode.mode);
        self.state.add_dir_entry(self.ino, &mut parent, name, target_ino, ft)?;
        self.state.write_inode_back(self.ino, &parent)?;
        target_inode.nlink = target_inode.nlink.saturating_add(1);
        self.state.write_inode_back(target_ino, &target_inode)?;
        Ok(())
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        if self.state.readonly {
            return Err(Errno::EROFS);
        }
        let mut parent = self.inode_disk()?;
        if mode_to_file_type(parent.mode) != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        let (child_ino, entry_off) = self.state.lookup_name(&parent, name)?.ok_or(Errno::ENOENT)?;
        let mut child = self.state.read_inode(child_ino)?;
        let child_type = mode_to_file_type(child.mode);
        if child_type == FileType::Directory && !self.state.dir_is_empty(&child)? {
            return Err(Errno::ENOTEMPTY);
        }

        self.state.mark_dirent_unused(self.ino, &mut parent, entry_off)?;
        self.state.write_inode_back(self.ino, &parent)?;

        if child.nlink > 0 {
            child.nlink -= 1;
        }
        self.state.write_inode_back(child_ino, &child)?;

        if child_type == FileType::Directory {
            if parent.nlink > 0 {
                parent.nlink -= 1;
            }
            self.state.write_inode_back(self.ino, &parent)?;
        }
        Ok(())
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        if self.state.readonly {
            return Err(Errno::EROFS);
        }
        let mut inode = self.inode_disk()?;
        if mode_to_file_type(inode.mode) != FileType::Symlink {
            return Err(Errno::EINVAL);
        }

        let data = target.as_bytes();
        if data.len() <= 60 {
            inode.block.fill(0);
            inode.block[..data.len()].copy_from_slice(data);
            inode.size = data.len() as u64;
            inode.blocks_512 = 0;
            self.state.write_inode_back(self.ino, &inode)?;
            return Ok(());
        }

        inode.size = 0;
        let _ = self.state.write_file_at(self.ino, &mut inode, data, 0)?;
        Ok(())
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let inode = self.inode_disk()?;
        if mode_to_file_type(inode.mode) == FileType::Directory {
            return Err(Errno::EISDIR);
        }
        let n = self.state.read_file_at(&inode, buf, offset as u64)?;
        self.overlay_cached_writes(&mut buf[..n], offset as u64);
        Ok(n)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        if self.state.readonly {
            return Err(Errno::EROFS);
        }
        let inode = self.inode_disk()?;
        if mode_to_file_type(inode.mode) == FileType::Directory {
            return Err(Errno::EISDIR);
        }
        let end = (offset as u64).saturating_add(buf.len() as u64);
        if end > inode.size {
            // Extending write: flush cached overwrites first, then write through
            self.flush_cached_writes()?;
            let mut inode = self.inode_disk()?;
            return self.state.write_file_at(self.ino, &mut inode, buf, offset as u64);
        }

        self.write_cache.lock().push(DirtyWrite {
            offset: offset as u64,
            data: buf.to_vec(),
        });
        Ok(buf.len())
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let inode = self.inode_disk()?;
        if mode_to_file_type(inode.mode) != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        self.state.read_dirent_at(&inode, index)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        let inode = self.inode_disk()?;
        if mode_to_file_type(inode.mode) != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        match self.state.lookup_name(&inode, name)? {
            Some((ino, _)) => Ok(ino),
            None => Err(Errno::ENOENT),
        }
    }

    fn rename(&self, old_name: &str, new_parent: &Arc<dyn InodeOps>, new_name: &str) -> SysResult<()> {
        if self.state.readonly {
            return Err(Errno::EROFS);
        }
        let new_parent = new_parent
            .downcast_ref::<Ext4Inode>()
            .ok_or(Errno::EXDEV)?;
        if !Arc::ptr_eq(&self.state, &new_parent.state) {
            return Err(Errno::EXDEV);
        }

        let mut old_dir = self.inode_disk()?;
        if mode_to_file_type(old_dir.mode) != FileType::Directory {
            return Err(Errno::ENOTDIR);
        }
        let (child_ino, old_entry_off) = self
            .state
            .lookup_name(&old_dir, old_name)?
            .ok_or(Errno::ENOENT)?;
        let mut child = self.state.read_inode(child_ino)?;
        let child_is_dir = mode_to_file_type(child.mode) == FileType::Directory;
        let ft = mode_to_dirent_type(child.mode);

        let same_dir = self.ino == new_parent.ino;
        let mut new_dir = if same_dir {
            old_dir.clone()
        } else {
            let d = new_parent.inode_disk()?;
            if mode_to_file_type(d.mode) != FileType::Directory {
                return Err(Errno::ENOTDIR);
            }
            d
        };

        // Handle existing target
        if let Some((target_ino, target_off)) = self.state.lookup_name(&new_dir, new_name)? {
            let mut target = self.state.read_inode(target_ino)?;
            let target_is_dir = mode_to_file_type(target.mode) == FileType::Directory;
            if child_is_dir && !target_is_dir {
                return Err(Errno::ENOTDIR);
            }
            if !child_is_dir && target_is_dir {
                return Err(Errno::EISDIR);
            }
            if target_is_dir && !self.state.dir_is_empty(&target)? {
                return Err(Errno::ENOTEMPTY);
            }
            // Remove existing target entry
            self.state
                .mark_dirent_unused(new_parent.ino, &mut new_dir, target_off)?;
            if target.nlink > 0 {
                target.nlink -= 1;
            }
            self.state.write_inode_back(target_ino, &target)?;
            if target_is_dir && new_dir.nlink > 0 {
                new_dir.nlink -= 1;
            }
        }

        // Add new entry, then remove old (safe order for crash consistency)
        self.state
            .add_dir_entry(new_parent.ino, &mut new_dir, new_name, child_ino, ft)?;
        self.state.write_inode_back(new_parent.ino, &new_dir)?;

        if same_dir {
            old_dir = new_dir;
        }
        self.state
            .mark_dirent_unused(self.ino, &mut old_dir, old_entry_off)?;
        self.state.write_inode_back(self.ino, &old_dir)?;

        // Cross-directory: update ".." and nlink counts
        if !same_dir && child_is_dir {
            if let Some((_, dotdot_off)) = self.state.lookup_name(&child, "..")? {
                let dot_len = dirent_need_len(2);
                let mut dotdot = vec![0u8; dot_len];
                dotdot[0..4].copy_from_slice(&new_parent.ino.to_le_bytes());
                dotdot[4..6].copy_from_slice(&(dot_len as u16).to_le_bytes());
                dotdot[6] = 2;
                dotdot[7] = 2;
                dotdot[8..10].copy_from_slice(b"..");
                self.state
                    .write_file_at(child_ino, &mut child, &dotdot, dotdot_off as u64)?;
            }
            // old parent loses a subdir, new parent gains one
            if old_dir.nlink > 0 {
                old_dir.nlink -= 1;
                self.state.write_inode_back(self.ino, &old_dir)?;
            }
            let mut new_dir = new_parent.inode_disk()?;
            new_dir.nlink = new_dir.nlink.saturating_add(1);
            self.state.write_inode_back(new_parent.ino, &new_dir)?;
        }

        Ok(())
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let inode = self.inode_disk()?;
        if mode_to_file_type(inode.mode) != FileType::Symlink {
            return Ok(None);
        }

        let target_len = min(buf.len(), inode.size as usize);
        if inode.blocks_512 == 0 && inode.size <= 60 {
            buf[..target_len].copy_from_slice(&inode.block[..target_len]);
            return Ok(Some(target_len));
        }

        let n = self.state.read_file_at(&inode, &mut buf[..target_len], 0)?;
        Ok(Some(n))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(self.inode_disk()?.size)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(self.inode_disk()?.mode as u32))
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        self.with_mut_inode(|inode| {
            let current = inode.mode as u32;
            let file_type = current & Mode::S_IFMT.bits();
            let mode_bits = (mode.bits() & 0o7777) | file_type;
            inode.mode = mode_bits as u16;
            Ok(())
        })
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        let inode = self.inode_disk()?;
        Ok((inode.uid, inode.gid))
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        self.with_mut_inode(|inode| {
            if let Some(new_uid) = uid {
                inode.uid = new_uid;
            }
            if let Some(new_gid) = gid {
                inode.gid = new_gid;
            }
            let mut mode = inode.mode as u32;
            mode &= !(Mode::S_ISUID | Mode::S_ISGID).bits();
            inode.mode = mode as u16;
            Ok(())
        })
    }

    fn inode_type(&self) -> SysResult<FileType> {
        Ok(mode_to_file_type(self.inode_disk()?.mode))
    }

    fn sync(&self) -> SysResult<()> {
        self.flush_cached_writes()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let inode = self.inode_disk()?;
        let mut st = FileStat::default();
        st.st_ino = self.ino as u64;
        st.st_mode = inode.mode as u32;
        st.st_nlink = inode.nlink as u32;
        st.st_uid = inode.uid;
        st.st_gid = inode.gid;
        st.st_size = inode.size as i64;
        st.st_blksize = self.state.sb.block_size as i32;
        st.st_blocks = inode.blocks_512;
        st.st_atime_sec = inode.atime as i64;
        st.st_mtime_sec = inode.mtime as i64;
        st.st_ctime_sec = inode.ctime as i64;
        Ok(st)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        if self.state.readonly {
            return Err(Errno::EROFS);
        }
        self.flush_cached_writes()?;
        let mut inode = self.inode_disk()?;
        let old_size = inode.size;
        if new_size == old_size {
            return Ok(());
        }
        if mode_to_file_type(inode.mode) != FileType::Regular {
            return Err(Errno::EINVAL);
        }

        let bs = self.state.block_size() as u64;
        if new_size < old_size {
            // Shrink: zero partial tail block, then free blocks beyond new size
            if new_size > 0 && (new_size % bs) != 0 {
                let tail_lblk = (new_size / bs) as u32;
                let tail_off = (new_size % bs) as usize;
                if let Some(pblk) = self.state.extent_map_lblk(&inode, tail_lblk)? {
                    let blk_base = (pblk as usize)
                        .checked_mul(bs as usize)
                        .ok_or(Errno::EINVAL)?;
                    let zeros = vec![0u8; bs as usize - tail_off];
                    self.state.write_bytes(blk_base + tail_off, &zeros)?;
                }
            }
            let keep_lblks = new_size.div_ceil(bs) as u32;
            self.state.truncate_extents(&mut inode, keep_lblks)?;
        }
        // Grow: simply increase size (blocks allocated on write, holes read as zero)

        inode.size = new_size;
        self.state.write_inode_back(self.ino, &inode)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        self.with_mut_inode(|inode| {
            inode.atime = time.as_secs().min(u32::MAX as u64) as u32;
            Ok(())
        })
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        self.with_mut_inode(|inode| {
            inode.mtime = time.as_secs().min(u32::MAX as u64) as u32;
            Ok(())
        })
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        self.with_mut_inode(|inode| {
            inode.ctime = time.as_secs().min(u32::MAX as u64) as u32;
            Ok(())
        })
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        let _ = self.flush_cached_writes();
    }
}
