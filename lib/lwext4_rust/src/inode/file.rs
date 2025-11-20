use core::{
    mem::{self, offset_of},
    slice,
};

use super::InodeRef;

use crate::{
    Ext4Result, InodeType, SystemHal, WritebackGuard, error::Context, ffi::*, util::get_block_size,
};

fn take<'a>(buf: &mut &'a [u8], cnt: usize) -> &'a [u8] {
    let (first, rem) = buf.split_at(cnt.min(buf.len()));
    *buf = rem;
    first
}
fn take_mut<'a>(buf: &mut &'a mut [u8], cnt: usize) -> &'a mut [u8] {
    // use mem::take to circumvent lifetime issues
    let pos = cnt.min(buf.len());
    let (first, rem) = mem::take(buf).split_at_mut(pos);
    *buf = rem;
    first
}

impl<Hal: SystemHal> InodeRef<Hal> {
    fn get_inode_fblock(&mut self, block: u32) -> Ext4Result<u64> {
        unsafe {
            let mut fblock = 0u64;
            ext4_fs_get_inode_dblk_idx(self.inner.as_mut(), block, &mut fblock, true)
                .context("ext4_fs_get_inode_dblk_idx")?;
            Ok(fblock)
        }
    }
    fn init_inode_fblock(&mut self, block: u32) -> Ext4Result<u64> {
        unsafe {
            let mut fblock = 0u64;
            ext4_fs_init_inode_dblk_idx(self.inner.as_mut(), block, &mut fblock)
                .context("ext4_fs_init_inode_dblk_idx")?;
            Ok(fblock)
        }
    }
    fn append_inode_fblock(&mut self) -> Ext4Result<(u64, u32)> {
        unsafe {
            let mut fblock = 0u64;
            let mut block = 0u32;
            ext4_fs_append_inode_dblk(self.inner.as_mut(), &mut fblock, &mut block)
                .context("ext4_fs_append_inode_dblk_idx")?;
            Ok((fblock, block))
        }
    }

    fn read_bytes(&mut self, offset: u64, buf: &mut [u8]) -> Ext4Result<()> {
        unsafe {
            let bdev = (*self.inner.fs).bdev;
            ext4_block_readbytes(bdev, offset, buf.as_mut_ptr() as _, buf.len() as _)
                .context("ext4_block_readbytes")
        }
    }
    fn write_bytes(&mut self, offset: u64, buf: &[u8]) -> Ext4Result<()> {
        unsafe {
            let bdev = (*self.inner.fs).bdev;
            ext4_block_writebytes(bdev, offset, buf.as_ptr() as _, buf.len() as _)
                .context("ext4_block_writebytes")
        }
    }

    pub fn read_at(&mut self, mut buf: &mut [u8], pos: u64) -> Ext4Result<usize> {
        unsafe {
            let file_size = self.size();
            let block_size = get_block_size(self.superblock());
            let bdev = (*self.inner.fs).bdev;

            if pos >= file_size || buf.is_empty() {
                return Ok(0);
            }
            let to_be_read = buf.len().min((file_size - pos) as usize);
            buf = &mut buf[..to_be_read];

            let inode = self.raw_inode();

            // symlink inline data
            if self.inode_type() == InodeType::Symlink && file_size < size_of::<[u32; 15]>() as u64
            {
                let content = (inode as *const _ as *const u8).add(offset_of!(ext4_inode, blocks));
                let buf = take_mut(&mut buf, (file_size - pos) as usize);
                buf.copy_from_slice(slice::from_raw_parts(content.add(pos as usize), buf.len()));
            }

            let mut block_start = (pos / block_size as u64) as u32;
            // This is inclusive!
            let block_end = ((pos + buf.len() as u64).min(file_size) / block_size as u64) as u32;

            let offset = pos % block_size as u64;
            if offset > 0 {
                let buf = take_mut(&mut buf, block_size as usize - offset as usize);
                let fblock = self.get_inode_fblock(block_start)?;
                if fblock != 0 {
                    self.read_bytes(fblock * block_size as u64 + offset, buf)?;
                } else {
                    buf.fill(0);
                }
                block_start += 1;
            }

            let guard = WritebackGuard::new(bdev);

            // Each block corresponds to a fblock, and we can read multiple
            // fblocks at once if they are consecutive.
            let mut fblock_start = 0;
            let mut fblock_count = 0;

            let flush_fblock_segment = |buf: &mut &mut [u8], start: u64, count: u32| {
                if count == 0 {
                    return Ok(());
                }
                let buf = take_mut(buf, count as usize * block_size as usize);
                ext4_blocks_get_direct(bdev, buf.as_mut_ptr() as _, start, count)
                    .context("ext4_blocks_get_direct")
            };
            for block in block_start..block_end {
                let fblock = self.get_inode_fblock(block)?;
                if fblock != fblock_start + fblock_count as u64 {
                    flush_fblock_segment(&mut buf, fblock_start, fblock_count)?;
                    fblock_start = fblock;
                    fblock_count = 0;
                }

                if fblock == 0 {
                    take_mut(&mut buf, block_size as usize).fill(0);
                } else {
                    fblock_count += 1;
                }
            }
            flush_fblock_segment(&mut buf, fblock_start, fblock_count)?;

            drop(guard);

            assert!(buf.len() < block_size as usize);
            if !buf.is_empty() {
                let fblock = self.get_inode_fblock(block_end)?;
                if fblock != 0 {
                    self.read_bytes(fblock * block_size as u64, buf)?;
                } else {
                    buf.fill(0);
                }
            }

            Ok(to_be_read)
        }
    }

    pub fn write_at(&mut self, mut buf: &[u8], pos: u64) -> Ext4Result<usize> {
        unsafe {
            let mut file_size = self.size();
            if pos > file_size {
                self.set_len(pos)?;
                // If we extend the file, we need to update the file size.
                file_size = self.size();
            }

            let block_size = get_block_size(self.superblock());
            let block_count = file_size.div_ceil(block_size as u64) as u32;
            let bdev = (*self.inner.fs).bdev;

            if buf.is_empty() {
                return Ok(0);
            }
            let to_be_written = buf.len();

            // TODO: symlink?

            let get_fblock = |this: &mut Self, block: u32| -> Ext4Result<u64> {
                if block < block_count {
                    this.init_inode_fblock(block)
                } else {
                    let (fblock, new_block) = this.append_inode_fblock()?;
                    assert_eq!(block, new_block);
                    Ok(fblock)
                }
            };

            let mut block_start = (pos / block_size as u64) as u32;
            // This is inclusive!
            let block_end = ((pos + buf.len() as u64) / block_size as u64) as u32;

            let offset = pos % block_size as u64;
            if offset > 0 {
                let buf = take(&mut buf, block_size as usize - offset as usize);
                let fblock = get_fblock(self, block_start)?;
                self.write_bytes(fblock * block_size as u64 + offset, buf)?;
                block_start += 1;
            }

            let mut fblock_start = 0;
            let mut fblock_count = 0;

            let flush_fblock_segment = |buf: &mut &[u8], start: u64, count: u32| {
                if count == 0 {
                    return Ok(());
                }
                let buf = take(buf, count as usize * block_size as usize);
                ext4_blocks_set_direct(bdev, buf.as_ptr() as _, start, count)
                    .context("ext4_blocks_set_direct")
            };
            for block in block_start..block_end {
                let fblock = get_fblock(self, block)?;
                if fblock != fblock_start + fblock_count as u64 {
                    flush_fblock_segment(&mut buf, fblock_start, fblock_count)?;
                    fblock_start = fblock;
                    fblock_count = 0;
                }
                fblock_count += 1;
            }
            flush_fblock_segment(&mut buf, fblock_start, fblock_count)?;

            assert!(buf.len() < block_size as usize);
            if !buf.is_empty() {
                let fblock = get_fblock(self, block_end)?;
                self.write_bytes(fblock * block_size as u64, buf)?;
            }

            let end = pos + to_be_written as u64;
            if end > file_size {
                ext4_inode_set_size(self.inner.inode, end);
                self.mark_dirty();
            }

            Ok(to_be_written)
        }
    }

    pub fn truncate(&mut self, size: u64) -> Ext4Result<()> {
        unsafe {
            let bdev = (*self.inner.fs).bdev;
            let _guard = WritebackGuard::new(bdev);
            ext4_fs_truncate_inode(self.inner.as_mut(), size).context("ext4_fs_truncate_inode")
        }
    }

    pub fn set_symlink(&mut self, target: &[u8]) -> Ext4Result<()> {
        let block_size = get_block_size(self.superblock());
        if target.len() > block_size as usize {
            // ENAMETOOLONG
            return 36.context("symlink too long");
        }

        unsafe {
            if target.len() < size_of::<u32>() * EXT4_INODE_BLOCKS as usize {
                let ptr = (self.inner.inode as *mut u8).add(offset_of!(ext4_inode, blocks));
                slice::from_raw_parts_mut(ptr, target.len()).copy_from_slice(target);
                ext4_inode_clear_flag(self.inner.inode, EXT4_INODE_FLAG_EXTENTS);
            } else {
                ext4_fs_inode_blocks_init(self.inner.fs, self.inner.as_mut());
                let mut fblock: u64 = 0;
                let mut sblock: u32 = 0;
                ext4_fs_append_inode_dblk(self.inner.as_mut(), &mut fblock, &mut sblock)
                    .context("ext4_fs_append_inode_dblk")?;

                let off = fblock * block_size as u64;
                self.write_bytes(off, target)?;
            }
            ext4_inode_set_size(self.inner.inode, target.len() as u64);
        }

        Ok(())
    }

    pub fn set_len(&mut self, len: u64) -> Ext4Result<()> {
        static EMPTY: [u8; 4096] = [0; 4096];

        let cur_len = self.size();
        if len < cur_len {
            self.truncate(len)?;
        } else if len > cur_len {
            // TODO: correct implementation
            let block_size = get_block_size(self.superblock());
            let old_blocks = cur_len.div_ceil(block_size as u64) as u32;
            let new_blocks = len.div_ceil(block_size as u64) as u32;
            for block in old_blocks..new_blocks {
                let (fblock, new_block) = self.append_inode_fblock()?;
                assert_eq!(block, new_block);
                self.write_bytes(fblock * block_size as u64, &EMPTY[..block_size as usize])?;
            }

            // Clear the last block extended part
            let old_last_block = (cur_len / block_size as u64) as u32;
            let old_block_start = (cur_len - (old_last_block as u64 * block_size as u64)) as usize;
            let fblock = self.init_inode_fblock(old_last_block)?;
            assert!(fblock != 0, "fblock should not be zero");
            let length = block_size as usize - old_block_start;
            self.write_bytes(
                fblock * block_size as u64 + old_block_start as u64,
                &EMPTY[..length],
            )?;

            unsafe {
                ext4_inode_set_size(self.inner.inode, len);
            }
            self.mark_dirty();
        }
        Ok(())
    }
}
