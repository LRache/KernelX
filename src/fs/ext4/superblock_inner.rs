pub(super) use ext4_rs::Ext4 as SuperBlockInner;
use ext4_rs::{Ext4InodeRef, Ext4DirSearchResult, Ext4DirEntry, Ext4DirEntryTail, Ext4Fsblk, Block};
use ext4_rs::{BLOCK_SIZE, EXT4_MAX_FILE_SIZE};
use core::cmp::{min, max};
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
        let mut write_buf_len = write_buf.len();
        if write_buf_len == 0 {
            return Ok(0);
        }

        // Get the file size
        let file_size = inode_ref.inode.size();
        // log::trace!("[Write] Starting write - inode: {}, offset: {}, size: {}, current file size: {}", 
        //     inode, offset, write_buf_len, file_size);

        // Calculate the start and end block index
        let iblock_start = offset / BLOCK_SIZE;
        let iblock_last = (offset + write_buf_len + BLOCK_SIZE - 1) / BLOCK_SIZE;
        let total_blocks_needed = iblock_last - iblock_start;

        // start block index
        let mut iblk_idx = iblock_start;
        let ifile_blocks = (file_size + BLOCK_SIZE as u64 - 1) / BLOCK_SIZE as u64;

        // Calculate the unaligned size
        let unaligned = offset % BLOCK_SIZE;
        if unaligned > 0 {
            // log::trace!("[Alignment] Unaligned start: {} bytes", unaligned);
        }

        // Buffer to keep track of written bytes
        let mut written = 0;
        let mut total_blocks = 0;
        let mut new_blocks = 0;

        // Start bgid for block allocation  
        let mut start_bgid = 1;

        // Pre-allocate blocks if needed
        let blocks_to_allocate = if iblk_idx >= ifile_blocks as usize {
            total_blocks_needed
        } else {
            max(0, total_blocks_needed - (ifile_blocks as usize - iblk_idx))
        };

        if blocks_to_allocate > 0 {
            // log::trace!("[Pre-allocation] Allocating {} blocks", blocks_to_allocate);
            
            // 使用append_inode_pblk_batch进行批量块分配
            let allocated_blocks = self.append_inode_pblk_batch(inode_ref, &mut start_bgid, blocks_to_allocate).map_err(map_error)?;
            
            // If we couldn't allocate all blocks, adjust the write size
            if allocated_blocks.len() < blocks_to_allocate {
                // log::trace!("[Write] Could only allocate {} out of {} blocks", allocated_blocks.len(), blocks_to_allocate);
                
                // Calculate new write size based on allocated blocks
                let max_write_size = allocated_blocks.len() * BLOCK_SIZE;
                let adjusted_write_size = if unaligned > 0 {
                    // For unaligned writes, we need to account for the unaligned portion
                    if allocated_blocks.len() > 0 {
                        let first_block_available = BLOCK_SIZE - unaligned;
                        let remaining_blocks_available = (allocated_blocks.len() - 1) * BLOCK_SIZE;
                        first_block_available + remaining_blocks_available
                    } else {
                        0
                    }
                } else {
                    max_write_size
                };
                
                if adjusted_write_size == 0 {
                    // log::error!("[Write] No space available for write after block allocation");
                    // return return_errno_with_message!(Errno::ENOSPC, "No blocks available for write");
                    return Err(Errno::ENOSPC);
                }
                
                // Update write size
                write_buf_len = min(write_buf_len, adjusted_write_size);
                // log::trace!("[Write] Adjusted write size from {} to {} bytes", write_buf.len(), write_buf_len);
            }
            
            new_blocks += allocated_blocks.len();
        }

        // Verify we have enough blocks for the write
        let required_blocks = (write_buf_len + BLOCK_SIZE - 1) / BLOCK_SIZE;
        let available_blocks = if iblk_idx >= ifile_blocks as usize {
            new_blocks
        } else {
            (ifile_blocks as usize - iblk_idx) + new_blocks
        };

        if available_blocks < required_blocks {
            // log::error!("[Write] Not enough blocks available: required {}, available {}", 
            //     required_blocks, available_blocks);
            // return return_errno_with_message!(Errno::ENOSPC, "Not enough blocks available for write");
            return Err(Errno::ENOSPC);
        }

        // Unaligned write
        if unaligned > 0 && written < write_buf_len {
            let len = min(write_buf_len, BLOCK_SIZE - unaligned);
            // log::trace!("[Unaligned Write] Writing {} bytes", len);
            
            // Get the physical block id
            let pblock_idx = match self.get_pblock_idx(&inode_ref, iblk_idx as u32) {
                Ok(idx) => idx,
                Err(e) => {
                    // log::error!("[Write] Failed to get physical block for logical block {}: {:?}", iblk_idx, e);
                    // return Err(e);
                    return Err(map_error(e));
                }
            };
            total_blocks += 1;

            let mut block = Block::load(&self.block_device, pblock_idx as usize * BLOCK_SIZE);
            
            // Read existing data if needed
            if unaligned > 0 || len < BLOCK_SIZE {
                let existing_data = self.block_device.read_offset(pblock_idx as usize * BLOCK_SIZE);
                block.data.copy_from_slice(&existing_data);
            }
            
            block.write_offset(unaligned, &write_buf[..len], len);

            // Verify write
            block.sync_blk_to_disk(&self.block_device);
            let verify_block = Block::load(&self.block_device, pblock_idx as usize * BLOCK_SIZE);
            if verify_block.data[unaligned..unaligned + len] != write_buf[..len] {
                // log::error!("[Write] Verification failed for unaligned write at block {}", pblock_idx);
                return Err(Errno::EIO);
            }
            drop(block);
            drop(verify_block);

            written += len;
            iblk_idx += 1;
        }

        // Aligned write
        let mut aligned_blocks = 0;
        // log::info!("[Aligned Write] Starting aligned writes for {} blocks", (write_buf_len - written + BLOCK_SIZE - 1) / BLOCK_SIZE);
        
        while written < write_buf_len {
            aligned_blocks += 1;
            
            // Get the physical block id
            let pblock_idx = match self.get_pblock_idx(&inode_ref, iblk_idx as u32) {
                Ok(idx) => idx,
                Err(e) => {
                    // log::error!("[Write] Failed to get physical block for logical block {}: {:?}", iblk_idx, e);
                    return Err(map_error(e));
                }
            };
            total_blocks += 1;

            let block_offset = pblock_idx as usize * BLOCK_SIZE;
            let mut block = Block::load(&self.block_device, block_offset);
            let write_size = min(BLOCK_SIZE, write_buf_len - written);
            
            // For partial block writes, read existing data first
            if write_size < BLOCK_SIZE {
                let existing_data = self.block_device.read_offset(block_offset);
                block.data.copy_from_slice(&existing_data);
            }
            
            block.write_offset(0, &write_buf[written..written + write_size], write_size);

            // Verify write
            block.sync_blk_to_disk(&self.block_device);
            let verify_block = Block::load(&self.block_device, block_offset);
            if verify_block.data[..write_size] != write_buf[written..written + write_size] {
                // log::error!("[Write] Verification failed for aligned write at block {}", pblock_idx);
                // return return_errno_with_message!(Errno::EIO, "Write verification failed");
                return Err(Errno::EIO);
            }
            drop(block);
            drop(verify_block);
            
            written += write_size;
            iblk_idx += 1;

            // if aligned_blocks % 1000 == 0 {
            //     log::trace!("[Progress] Written {} blocks, {} bytes", aligned_blocks, written);
            // }
        }
        
        // Update file size if necessary
        let new_size = offset + written;
        if new_size > file_size as usize {
            // log::trace!("[Write] Updating file size from {} to {}", file_size, new_size);
            
            // Verify the new size is valid
            if new_size > EXT4_MAX_FILE_SIZE as usize {
                // log::error!("[Write] New file size {} exceeds maximum allowed size", new_size);
                // return return_errno_with_message!(Errno::EFBIG, "File size too large");
                return Err(Errno::EFBIG);
            }
            
            inode_ref.inode.set_size(new_size as u64);
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
