pub(super) use ext4_rs::Ext4 as SuperBlockInner;
use ext4_rs::{Ext4InodeRef, Ext4DirSearchResult, Ext4DirEntry, Ext4DirEntryTail, Ext4Fsblk, Block};
use ext4_rs::BLOCK_SIZE;
use core::cmp::min;
use alloc::vec::Vec;

use crate::kernel::errno::{Errno, SysResult};

pub(super) fn map_error(e: ext4_rs::Ext4Error) -> Errno {
    match e.error() {
        ext4_rs::Errno::EIO    => Errno::EIO,
        ext4_rs::Errno::EEXIST => Errno::EEXIST,
        ext4_rs::Errno::ENOENT => Errno::ENOENT,
        ext4_rs::Errno::ENOSPC => Errno::ENOSPC,
        _ => Errno::EIO,
    }
}

pub(super) trait SuperBlockInnerExt {
    fn create_ref(&self, inode_ref: &mut Ext4InodeRef, name: &str, mode: u16) -> SysResult<()>;
    fn unlink_ref(&self, parent_inode_ref: &mut Ext4InodeRef, child_inode_ref: &mut Ext4InodeRef, name: &str) -> SysResult<()>;

    fn readat_ref(&self, inode_ref: &Ext4InodeRef, offset: usize, buf: &mut [u8]) -> SysResult<usize>;
    fn writeat_ref(&self, inode_ref: &mut Ext4InodeRef, offset: usize, buf: &[u8]) -> SysResult<usize>;

    fn dir_find_entry_ref(&self, inode_ref: &Ext4InodeRef, name: &str) -> SysResult<Option<Ext4DirSearchResult>>;
    fn dir_get_entries_ref(&self, inode_ref: &Ext4InodeRef) -> SysResult<Vec<Ext4DirEntry>>;
}

impl SuperBlockInnerExt for SuperBlockInner {
    fn create_ref(&self, parent_inode_ref: &mut Ext4InodeRef, name: &str, inode_mode: u16) -> SysResult<()> {
        // let mut child_inode_ref = self.create_inode(inode_mode)?;
        let init_child_ref = self.create_inode(inode_mode).map_err(map_error)?;

        self.write_back_inode_without_csum(&init_child_ref);
        // load new
        let mut child_inode_ref = self.get_inode_ref(init_child_ref.inode_num);

        self.link(parent_inode_ref, &mut child_inode_ref, name).map_err(map_error)?;

        self.write_back_inode(parent_inode_ref);
        self.write_back_inode(&mut child_inode_ref);

        Ok(())
    }

    fn unlink_ref(&self, parent_inode_ref: &mut Ext4InodeRef, child_inode_ref: &mut Ext4InodeRef, name: &str) -> SysResult<()> {
        self.unlink(parent_inode_ref, child_inode_ref, name).map_err(map_error)?;
        Ok(())
    }
    
    fn readat_ref(&self, inode_ref: &Ext4InodeRef, offset: usize, read_buf: &mut [u8]) -> SysResult<usize> {
        let mut read_buf_len = read_buf.len();
        if read_buf_len == 0 {
            return Ok(0);
        }

        // get the inode reference
        let file_size = inode_ref.inode.size();
        let total_blocks = (file_size + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64;

        // if the offset is greater than the file size, return 0
        if offset >= file_size as usize {
            return Ok(0);
        }

        // adjust the read buffer size if the read buffer size is greater than the file size
        if offset + read_buf_len > file_size as usize {
            read_buf_len = file_size as usize - offset;
            // log::trace!("[Read] Adjusted read size to {} bytes to not exceed file size (offset: {}, file_size: {})", 
            //     read_buf_len, offset, file_size);
        }

        // calculate the start block and unaligned size
        let iblock_start = offset / BLOCK_SIZE;
        let iblock_last = (offset + read_buf_len + BLOCK_SIZE - 1) / BLOCK_SIZE; // round up to include the last partial block
        let unaligned_start_offset = offset % BLOCK_SIZE;
        
        // Ensure we don't read beyond the last block
        let iblock_last = min(iblock_last, total_blocks as usize);
        

        // Buffer to keep track of read bytes
        let mut cursor = 0;
        let mut total_bytes_read = 0;
        let mut iblock = iblock_start;

        // Unaligned read at the beginning
        if unaligned_start_offset > 0 {
            let adjust_read_size = min(BLOCK_SIZE - unaligned_start_offset, read_buf_len);

            // get iblock physical block id
            let pblock_idx = match self.get_pblock_idx(&inode_ref, iblock as u32) {
                Ok(idx) => {
                    idx
                },
                Err(_) => {
                    // return_errno_with_message!(Errno::EIO, "Failed to get physical block for logical block");
                    return Err(Errno::EIO);
                }
            };

            // read data
            let data = self.block_device.read_offset(pblock_idx as usize * BLOCK_SIZE);

            // copy data to read buffer
            read_buf[cursor..cursor + adjust_read_size].copy_from_slice(
                &data[unaligned_start_offset..unaligned_start_offset + adjust_read_size],
            );

            // update cursor and total bytes read
            cursor += adjust_read_size;
            total_bytes_read += adjust_read_size;
            iblock += 1;
        }

        // Continue with full block reads
        while total_bytes_read < read_buf_len && iblock < iblock_last {
            let mut read_length = min(BLOCK_SIZE, read_buf_len - total_bytes_read);
            
            // Check if this is the last block of the file
            if iblock as u64 >= total_blocks - 1 {
                let remaining_bytes = file_size as usize - (iblock * BLOCK_SIZE);
                let actual_read_length = min(read_length, remaining_bytes);

                if actual_read_length < read_length {
                    read_length = actual_read_length;
                }
            }
            

            // get iblock physical block id
            let pblock_idx = match self.get_pblock_idx(&inode_ref, iblock as u32) {
                Ok(idx) => {
                    idx
                },
                Err(_) => {
                    // return_errno_with_message!(Errno::EIO, "Failed to get physical block for logical block");
                    return Err(Errno::EIO);
                }
            };

            // read data
            let data = self.block_device.read_offset(pblock_idx as usize * BLOCK_SIZE);
            // log::trace!("[Read] Read block data - physical_block: {}, data_len: {}", pblock_idx, data.len());

            // copy data to read buffer
            read_buf[cursor..cursor + read_length].copy_from_slice(&data[..read_length]);

            // update cursor and total bytes read
            cursor += read_length;
            total_bytes_read += read_length;
            iblock += 1;
        }

        Ok(total_bytes_read)
    }

    fn writeat_ref(&self, inode_ref: &mut Ext4InodeRef, offset: usize, write_buf: &[u8]) -> SysResult<usize> {
        // write buf is empty, return 0
        let write_buf_len = write_buf.len();
        if write_buf_len == 0 {
            return Ok(0);
        }

        // Get the file size
        let file_size = inode_ref.inode.size();

        // Calculate the start and end block index
        let iblock_start = offset / BLOCK_SIZE;
        let iblock_last = (offset + write_buf_len + BLOCK_SIZE - 1) / BLOCK_SIZE; // round up to include the last partial block

        // start block index
        let mut iblk_idx = iblock_start;
        let ifile_blocks = (file_size + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64;

        // Calculate the unaligned size
        let unaligned = offset % BLOCK_SIZE;

        // Buffer to keep track of written bytes
        let mut written = 0;

        // Start bgid
        let mut start_bgid = 1;

        // Unaligned write
        if unaligned > 0 {
            let len = min(write_buf_len, BLOCK_SIZE - unaligned);
            // Get the physical block id, if the block is not present, append a new block
            let pblock_idx = if iblk_idx < ifile_blocks as usize {
                self.get_pblock_idx(&inode_ref, iblk_idx as u32).map_err(map_error)?
            } else {
                // physical block not exist, append a new block
                self.append_inode_pblk_from(inode_ref, &mut start_bgid).map_err(map_error)?
            };

            let mut block =
                Block::load(&self.block_device, pblock_idx as usize * BLOCK_SIZE);

            block.write_offset(unaligned, &write_buf[..len], len);
            block.sync_blk_to_disk(self.block_device.clone());
            drop(block);


            written += len;
            iblk_idx += 1;
        }

        // Aligned write
        let mut fblock_start = 0;
        let mut fblock_count = 0;

        while written < write_buf_len {
            while iblk_idx < iblock_last && written < write_buf_len {
                // Get the physical block id, if the block is not present, append a new block
                let pblock_idx = if iblk_idx < ifile_blocks as usize {
                    self.get_pblock_idx(inode_ref, iblk_idx as u32).map_err(map_error)?
                } else {
                    // physical block not exist, append a new block
                    self.append_inode_pblk_from(inode_ref, &mut start_bgid).map_err(map_error)?
                };
                if fblock_start == 0 {
                    fblock_start = pblock_idx;
                }

                // Check if the block is contiguous
                if fblock_start + fblock_count != pblock_idx {
                    break;
                }

                fblock_count += 1;
                iblk_idx += 1;
            }

            // Write contiguous blocks at once
            let len = min(
                fblock_count as usize * BLOCK_SIZE,
                write_buf_len - written,
            );

            for i in 0..fblock_count {
                let block_offset = fblock_start as usize * BLOCK_SIZE + i as usize * BLOCK_SIZE;
                let mut block = Block::load(&self.block_device, block_offset);
                let write_size = min(BLOCK_SIZE, write_buf_len - written);
                block.write_offset(0, &write_buf[written..written + write_size], write_size);
                block.sync_blk_to_disk(self.block_device.clone());
                drop(block);
                written += write_size;
            }

            fblock_start = 0;
            fblock_count = 0;
        }

        // Final unaligned write if any
        if written < write_buf_len {
            let len = write_buf_len - written;
            // Get the physical block id, if the block is not present, append a new block
            let pblock_idx = if iblk_idx < ifile_blocks as usize {
                self.get_pblock_idx(inode_ref, iblk_idx as u32).map_err(map_error)?
            } else {
                // physical block not exist, append a new block
                self.append_inode_pblk(inode_ref).map_err(map_error)?
            };

            let mut block =
                Block::load(&self.block_device, pblock_idx as usize * BLOCK_SIZE);
            block.write_offset(0, &write_buf[written..], len);
            block.sync_blk_to_disk(self.block_device.clone());
            drop(block);

            written += len;
        }

        // Update file size if necessary
        if offset + write_buf_len > file_size as usize {
            // log::trace!("set file size {:x}", offset + write_buf_len);
            inode_ref
                .inode
                .set_size((offset + write_buf_len) as u64);

            self.write_back_inode(inode_ref);
        }

        Ok(written)
    }

    fn dir_find_entry_ref(&self, parent: &Ext4InodeRef, name: &str) -> SysResult<Option<Ext4DirSearchResult>> {
        if !parent.inode.is_dir() {
            // return_errno_with_message!(Errno::ENOTDIR, "Not a directory");
            return Err(Errno::ENOTDIR);
        }

        // start from the first logical block
        let mut iblock = 0;
        // physical block id
        let mut fblock: Ext4Fsblk = 0;

        // calculate total blocks
        let inode_size: u64 = parent.inode.size();
        let total_blocks: u64 = inode_size / BLOCK_SIZE as u64;

        // iterate all blocks
        while iblock < total_blocks {
            let search_path = self.find_extent(&parent, iblock as u32);

            if let Ok(path) = search_path {
                // get the last path
                let path = path.path.last().unwrap();

                // get physical block id
                fblock = path.pblock;

                // load physical block
                let mut ext4block =
                    Block::load(&self.block_device, fblock as usize * BLOCK_SIZE);

                let mut result = Ext4DirSearchResult::new(Ext4DirEntry::default());
                // find entry in block
                let r = self.dir_find_in_block(&ext4block, name, &mut result);

                if r.is_ok() {
                    result.pblock_id = fblock as usize;
                    return Ok(Some(result));
                }
            } else {
                return Ok(None);
            }
            // go to next block
            iblock += 1
        }

        // return_errno_with_message!(Errno::ENOENT, "dir search fail");
        return Ok(None);
    }

    fn dir_get_entries_ref(&self, inode_ref: &Ext4InodeRef) -> SysResult<Vec<Ext4DirEntry>> {
        if !inode_ref.inode.is_dir() {
            return Err(Errno::ENOTDIR);
        }

        // calculate total blocks
        let inode_size = inode_ref.inode.size();
        let total_blocks = inode_size / BLOCK_SIZE as u64;

        // start from the first logical block
        let mut iblock = 0;

        let mut entries = Vec::new();

        // iterate all blocks
        while iblock < total_blocks {
            // get physical block id of a logical block id
            let search_path = self.find_extent(&inode_ref, iblock as u32);

            if let Ok(path) = search_path {
                // get the last path
                let path = path.path.last().unwrap();

                // get physical block id
                let fblock = path.pblock;

                // load physical block
                let ext4block =
                    Block::load(&self.block_device, fblock as usize * BLOCK_SIZE);
                let mut offset = 0;

                // iterate all entries in a block
                while offset < BLOCK_SIZE - core::mem::size_of::<Ext4DirEntryTail>() {
                    let de: Ext4DirEntry = ext4block.read_offset_as(offset);
                    entries.push(de);
                    offset += de.entry_len() as usize;
                }
            }

            // go to next block
            iblock += 1;
        }
        
        Ok(entries)
    }
}
