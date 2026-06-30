use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::FileType;
use crate::fs::vfs::{evict_inode, find_cached_inode};
use crate::fs::{Dentry, Inode as VfsInode, InodeOps, Mode, Owner, VfsInode as VfsInodeWrapper};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{FileStat, Uid};
use crate::klib::{SleepLock, SpinLock};

use super::ondisk::{DirEntry2, Ext4InodeFlags, debug_errno, lookup_extent_lblk, lookup_lblk, ret_errno};
use super::{Context, Ext4Inode, ExtentLeaf};

const S_IFMT: u16 = 0xF000;
const S_IFDIR: u16 = 0x4000;
const S_IFREG: u16 = 0x8000;
const EXT_INIT_MAX_LEN: u16 = 32768;
const MAX_BULK_ALLOC_BLOCKS: usize = 128;

const EXT4_FT_REG_FILE: u8 = 1;
const EXT4_FT_DIR: u8 = 2;
const EXT4_FT_CHRDEV: u8 = 3;
const EXT4_FT_BLKDEV: u8 = 4;
const EXT4_FT_FIFO: u8 = 5;
const EXT4_FT_SOCK: u8 = 6;
const EXT4_FT_SYMLINK: u8 = 7;

pub struct Inode {
    context: Weak<SleepLock<Context>>,
    inode: SleepLock<Ext4Inode>,
    dents_cache: SpinLock<Option<Vec<DirResult>>>,
    deleted: AtomicBool,
}

impl Inode {
    pub fn new(context: Weak<SleepLock<Context>>, inode: Ext4Inode) -> Self {
        Self {
            context,
            inode: SleepLock::new(inode, "ext4_native::Inode::inode"),
            dents_cache: SpinLock::new(None, "ext4_native::Inode::dents_cache"),
            deleted: AtomicBool::new(false),
        }
    }

    fn refresh_cached_state(&self, inode: &Ext4Inode) {
        *self.inode.lock() = inode.clone();
    }

    fn invalidate_dir_cache(&self) {
        *self.dents_cache.lock() = None;
    }

    fn cached_dir_results(&self) -> Option<Vec<DirResult>> {
        self.dents_cache.lock().clone()
    }

    fn dir_results(&self, context: &Context, inode: &Ext4Inode) -> SysResult<Vec<DirResult>> {
        if let Some(entries) = self.cached_dir_results() {
            return Ok(entries);
        }

        let entries = read_dir_results_from_disk(context, inode)?;
        *self.dents_cache.lock() = Some(entries.clone());
        Ok(entries)
    }

    fn mark_deleted(&self, inode: &Ext4Inode) {
        self.refresh_cached_state(inode);
        self.invalidate_dir_cache();
        self.deleted.store(true, Ordering::Release);
    }
}

impl InodeOps for Inode {
    fn get_ino(&self) -> u32 {
        self.inode.lock().ino
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("readat: context has been dropped", Errno::EIO))?;
        let context = context.lock();
        let inode = self.inode.lock();

        let file_size = inode.i_size as usize;
        if offset >= file_size {
            return Ok(0);
        }
        let to_read = buf.len().min(file_size - offset);
        if to_read == 0 {
            return Ok(0);
        }

        if inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            // TODO: support inline data
            return ret_errno("readat: inline_data inode unsupported", Errno::EOPNOTSUPP);
        }
        if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            let block_size = context.block_size as usize;
            let mut copied = 0;
            while copied < to_read {
                let file_off = offset + copied;
                let lblk = u32::try_from(file_off / block_size)
                    .map_err(|_| debug_errno("readat: logical block number overflow", Errno::EFBIG))?;
                let in_block = file_off % block_size;
                let chunk = core::cmp::min(to_read - copied, block_size - in_block);
                let dst = &mut buf[copied..copied + chunk];

                match lookup_lblk(&context, &inode, lblk)? {
                    Some(pblk) => {
                        let data = context.read_fs_block(pblk)?;
                        dst.copy_from_slice(&data[in_block..in_block + chunk]);
                    }
                    None => dst.fill(0),
                }
                copied += chunk;
            }

            return Ok(copied);
        }

        let block_size = context.block_size as usize;
        let ino = inode.ino;
        let (_, extents, _) = context.extent_tree_snapshot(ino, &inode)?;

        let mut copied = 0;
        let mut extent_idx = 0usize;
        while copied < to_read {
            let file_off = offset + copied;
            let lblk = u32::try_from(file_off / block_size)
                .map_err(|_| debug_errno("readat: logical block number overflow", Errno::EFBIG))?;
            let in_block = file_off % block_size;
            let remaining = to_read - copied;

            advance_extent_cursor(&extents, &mut extent_idx, lblk);
            if in_block == 0 {
                if let Some(extent) = mapped_extent_for_read(&extents, extent_idx, lblk) {
                    let full_blocks = full_block_run_len(extent, lblk, remaining, block_size);
                    if full_blocks != 0 {
                        let start_pblk = extent.ee_start + (lblk - extent.ee_block) as u64;
                        let byte_len = full_blocks * block_size;
                        context.read_fs_blocks_into(start_pblk, &mut buf[copied..copied + byte_len])?;
                        copied += byte_len;
                        continue;
                    }
                } else {
                    let hole_blocks = hole_run_blocks(&extents, extent_idx, lblk);
                    let full_blocks = (remaining / block_size).min(hole_blocks);
                    if full_blocks != 0 {
                        let byte_len = full_blocks * block_size;
                        buf[copied..copied + byte_len].fill(0);
                        copied += byte_len;
                        continue;
                    }
                }
            }

            let chunk = core::cmp::min(remaining, block_size - in_block);
            let dst = &mut buf[copied..copied + chunk];
            match mapped_read_pblk(&extents, extent_idx, lblk) {
                Some(pblk) => {
                    let data = context.read_fs_block(pblk)?;
                    dst.copy_from_slice(&data[in_block..in_block + chunk]);
                }
                None => dst.fill(0),
            }
            copied += chunk;
        }

        Ok(copied)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("writeat: context has been dropped", Errno::EIO))?;
        let context = context.lock();
        let mut inode = self.inode.lock();

        let mode_type = inode.i_mode & S_IFMT;
        if mode_type == S_IFDIR {
            return ret_errno("writeat: inode is a directory", Errno::EISDIR);
        }
        if mode_type != S_IFREG {
            return ret_errno("writeat: inode is not a regular file", Errno::EINVAL);
        }
        if inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno("writeat: inline_data inode unsupported", Errno::EOPNOTSUPP);
        }
        if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("writeat: non-extent inode unsupported", Errno::EOPNOTSUPP);
        }

        let end = offset
            .checked_add(buf.len())
            .ok_or_else(|| debug_errno("writeat: offset+len overflow", Errno::EINVAL))?;

        let block_size = context.block_size as usize;
        let ino = inode.ino;
        let (tree_generation, mut extents, old_extent_blocks) = context.extent_tree_snapshot(ino, &inode)?;
        let old_extents = extents.clone();

        let mut written = 0;
        let mut extent_idx = 0usize;
        let mut extents_dirty = false;
        let mut metadata_dirty = false;
        let mut newly_allocated = Vec::new();
        let write_result: SysResult<()> = (|| {
            while written < buf.len() {
                let file_off = offset + written;
                let lblk_usize = file_off / block_size;
                let lblk = u32::try_from(lblk_usize)
                    .map_err(|_| debug_errno("writeat: logical block number overflow", Errno::EFBIG))?;
                let in_block = file_off % block_size;
                let remaining = buf.len() - written;

                advance_extent_cursor(&extents, &mut extent_idx, lblk);
                if in_block == 0 && remaining >= block_size {
                    if let Some(extent) = mapped_extent_for_write(&extents, extent_idx, lblk) {
                        let full_blocks = full_block_run_len(extent, lblk, remaining, block_size);
                        if full_blocks != 0 {
                            let byte_len = full_blocks * block_size;
                            let start_pblk = extent.ee_start + (lblk - extent.ee_block) as u64;
                            context.write_fs_blocks(start_pblk, &buf[written..written + byte_len])?;
                            written += byte_len;
                            continue;
                        }
                    } else {
                        let desired_blocks = (remaining / block_size)
                            .min(hole_run_blocks(&extents, extent_idx, lblk))
                            .min(MAX_BULK_ALLOC_BLOCKS);
                        if desired_blocks != 0 {
                            let (allocated_blocks, segments) = allocate_full_block_run(
                                &context,
                                &mut extents,
                                &mut extent_idx,
                                lblk,
                                desired_blocks,
                                &mut newly_allocated,
                            )?;
                            if allocated_blocks != 0 {
                                let mut buf_off = written;
                                for (start_pblk, block_count) in segments {
                                    let byte_len = block_count * block_size;
                                    context.write_fs_blocks(start_pblk, &buf[buf_off..buf_off + byte_len])?;
                                    buf_off += byte_len;
                                }
                                extents_dirty = true;
                                written += allocated_blocks * block_size;
                                continue;
                            }
                        }
                    }
                }

                let chunk = core::cmp::min(remaining, block_size - in_block);
                let mut new_block = false;
                let pblk = match mapped_write_pblk(&extents, extent_idx, lblk) {
                    Some(p) => p,
                    None => {
                        let new_pblk = context.alloc_block()?;
                        if let Err(err) = context.insert_extent_mapping(&mut extents, lblk, new_pblk) {
                            let _ = context.free_block(new_pblk);
                            return Err(err);
                        }
                        newly_allocated.push(new_pblk);
                        extents_dirty = true;
                        extent_idx = extent_idx.saturating_sub(1);
                        advance_extent_cursor(&extents, &mut extent_idx, lblk);
                        new_block = true;
                        new_pblk
                    }
                };

                let src = &buf[written..written + chunk];
                if chunk == block_size && in_block == 0 {
                    context.write_fs_block(pblk, src)?;
                } else {
                    let mut data = if new_block {
                        vec![0u8; block_size]
                    } else {
                        context.read_fs_block(pblk)?
                    };
                    data[in_block..in_block + chunk].copy_from_slice(src);
                    context.write_fs_block(pblk, &data)?;
                }

                written += chunk;
            }
            Ok(())
        })();
        if let Err(err) = write_result {
            for pblk in newly_allocated {
                let _ = context.free_block(pblk);
            }
            return Err(err);
        }

        if (end as u64) > inode.i_size {
            inode.i_size = end as u64;
            metadata_dirty = true;
        }
        if extents_dirty {
            if let Err(err) =
                context.replace_extent_tree(ino, &mut inode, tree_generation, &extents, &old_extent_blocks)
            {
                for pblk in newly_allocated {
                    let _ = context.free_block(pblk);
                }
                return Err(err);
            }
            context.update_i_blocks_from_extent_delta(&mut inode, &old_extents, old_extent_blocks.len(), &extents)?;
            metadata_dirty = true;
        }
        if metadata_dirty {
            context.write_inode(&mut inode)?;
        }

        Ok(written)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("truncate: context has been dropped", Errno::EIO))?;
        let context = context.lock();
        let mut inode = self.inode.lock();

        let mode_type = inode.i_mode & S_IFMT;
        if mode_type == S_IFDIR {
            return ret_errno("truncate: inode is a directory", Errno::EISDIR);
        }
        if mode_type != S_IFREG {
            return ret_errno("truncate: inode is not a regular file", Errno::EINVAL);
        }
        if inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno("truncate: inline_data inode unsupported", Errno::EOPNOTSUPP);
        }
        if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno("truncate: non-extent inode unsupported", Errno::EOPNOTSUPP);
        }

        let old_size = inode.i_size;
        if new_size == old_size {
            return Ok(());
        }

        let block_size = context.block_size as u64;
        let ino = inode.ino;
        let generation = inode.i_generation;

        if new_size < old_size {
            let first_drop_u64 = new_size.div_ceil(block_size);
            let first_drop_lblk = u32::try_from(first_drop_u64)
                .map_err(|_| debug_errno("truncate: first drop lblk overflow", Errno::EFBIG))?;

            context.remove_extent_range(ino, &mut inode, first_drop_lblk, u32::MAX)?;

            let tail_off = (new_size % block_size) as usize;
            if tail_off != 0 {
                let kept_lblk = u32::try_from(new_size / block_size)
                    .map_err(|_| debug_errno("truncate: kept lblk overflow", Errno::EFBIG))?;
                let root = context.parse_extent_root(&inode)?;
                if let Some(pblk) = lookup_extent_lblk(&context, root, ino, generation, kept_lblk)? {
                    let mut data = context.read_fs_block(pblk)?;
                    data[tail_off..].fill(0);
                    context.write_fs_block(pblk, &data)?;
                }
            }
        }

        inode.i_size = new_size;
        context.write_inode(&mut inode)?;
        Ok(())
    }

    fn sync(&self) -> SysResult<()> {
        if self.deleted.load(Ordering::Acquire) {
            return Ok(());
        }

        let context = self
            .context
            .upgrade()
            // .ok_or_else(|| debug_errno("sync: context has been dropped", Errno::EIO))?;
            .ok_or(Errno::EIO)?;
        let context = context.lock();
        let mut inode = self.inode.lock();
        context.write_inode(&mut inode)
    }

    fn type_name(&self) -> &'static str {
        "ext4_native"
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Self> {
        let mode_type = mode & Mode::S_IFMT;
        let (is_dir, file_type) = if mode_type == Mode::S_IFREG {
            (false, EXT4_FT_REG_FILE)
        } else if mode_type == Mode::S_IFDIR {
            (true, EXT4_FT_DIR)
        } else {
            return ret_errno("create: unsupported inode type", Errno::EOPNOTSUPP);
        };

        let name_bytes = name.as_bytes();
        if name_bytes.is_empty() || name_bytes.len() > u8::MAX as usize {
            return ret_errno("create: name is empty or too long", Errno::EINVAL);
        }

        match self.lookup(name) {
            Ok(_) => return ret_errno("create: name already exists", Errno::EEXIST),
            Err(Errno::ENOENT) => {}
            Err(err) => return Err(err),
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("create: context has been dropped", Errno::EIO))?;
        let context_lock = context.lock();
        let mut parent = self.inode.lock();

        ensure_dir_readable(&parent)?;
        if parent.i_flags.contains(Ext4InodeFlags::INDEX) {
            return ret_errno("create: htree indexed directory unsupported", Errno::EOPNOTSUPP);
        }

        let child_ino = context_lock.alloc_inode(is_dir)?;
        let mut child_block = None;
        let result = (|| -> SysResult<Self> {
            let mut child = context_lock.new_inode(
                child_ino,
                mode.bits() as u16,
                owner.uid as u16,
                owner.gid as u16,
                if is_dir { 2 } else { 1 },
            )?;

            if is_dir {
                let pblk = context_lock.alloc_block()?;
                child_block = Some(pblk);
                context_lock.insert_extent_1blk(child_ino, &mut child, 0, pblk)?;
                context_lock.init_dir_block(pblk, child_ino, child.i_generation, parent.ino)?;
                child.i_size = context_lock.block_size as u64;
                parent.i_links_count = parent
                    .i_links_count
                    .checked_add(1)
                    .ok_or_else(|| debug_errno("create: parent link count overflow", Errno::EIO))?;
            }

            context_lock.write_inode(&mut child)?;
            context_lock.insert_dirent(parent.ino, &mut parent, name_bytes, child_ino, file_type)?;

            Ok(Self::new(Arc::downgrade(&context), child))
        })();

        match result {
            Ok(inode) => {
                self.invalidate_dir_cache();
                Ok(inode)
            }
            Err(err) => {
                if let Some(pblk) = child_block {
                    let _ = context_lock.free_block(pblk);
                }
                let _ = context_lock.free_inode_bit(child_ino, is_dir);
                Err(err)
            }
        }
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        let name_bytes = name.as_bytes();
        if name_bytes.is_empty() || name_bytes.len() > u8::MAX as usize {
            return ret_errno("unlink: name is empty or too long", Errno::EINVAL);
        }
        if name == "." || name == ".." {
            return ret_errno("unlink: '.' and '..' cannot be unlinked", Errno::EINVAL);
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("unlink: context has been dropped", Errno::EIO))?;
        let context = context.lock();
        let mut parent = self.inode.lock();

        ensure_dir_writable(&parent, "unlink")?;

        let child_ino = lookup_name_in_dir(&context, &parent, name_bytes)?;
        let mut child = context.read_inode(child_ino)?;
        let mode_type = child.i_mode & S_IFMT;

        if mode_type == S_IFDIR {
            return ret_errno("unlink: target is a directory", Errno::EISDIR);
        }
        if child.i_links_count == 0 {
            return ret_errno("unlink: inode link count is already zero", Errno::EIO);
        }

        if child.i_links_count > 1 {
            let old_links = child.i_links_count;
            child.i_links_count -= 1;
            context.write_inode(&mut child)?;
            sync_cached_inode(context.fsno, &child);

            if let Err(err) = context.remove_dirent(parent.ino, &mut parent, name_bytes) {
                child.i_links_count = old_links;
                let _ = context.write_inode(&mut child);
                sync_cached_inode(context.fsno, &child);
                return Err(err);
            }

            self.invalidate_dir_cache();
            return Ok(());
        }

        ensure_unlink_cleanup_supported(&child)?;

        let fsno = context.fsno;
        context.remove_dirent(parent.ino, &mut parent, name_bytes)?;
        destroy_unlinked_inode(&context, &mut child, false)?;
        mark_cached_inode_deleted(fsno, &child);
        self.invalidate_dir_cache();
        drop(parent);
        drop(context);
        evict_inode(fsno, child_ino);
        Ok(())
    }

    fn rmdir(&self, name: &str) -> SysResult<()> {
        let name_bytes = name.as_bytes();
        if name_bytes.is_empty() || name_bytes.len() > u8::MAX as usize {
            return ret_errno("rmdir: name is empty or too long", Errno::EINVAL);
        }
        if name == "." || name == ".." {
            return ret_errno("rmdir: '.' and '..' cannot be removed", Errno::EINVAL);
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("rmdir: context has been dropped", Errno::EIO))?;
        let context = context.lock();
        let mut parent = self.inode.lock();

        ensure_dir_writable(&parent, "rmdir")?;

        let child_ino = lookup_name_in_dir(&context, &parent, name_bytes)?;
        let mut child = context.read_inode(child_ino)?;
        ensure_rmdir_cleanup_supported(&child)?;

        if !is_dir_empty(&context, &child)? {
            return ret_errno("rmdir: directory is not empty", Errno::ENOTEMPTY);
        }

        let old_parent_links = parent.i_links_count;
        let fsno = context.fsno;
        context.remove_dirent(parent.ino, &mut parent, name_bytes)?;
        parent.i_links_count = old_parent_links
            .checked_sub(1)
            .ok_or_else(|| debug_errno("rmdir: parent link count underflow", Errno::EIO))?;
        context.write_inode(&mut parent)?;
        destroy_unlinked_inode(&context, &mut child, true)?;
        mark_cached_inode_deleted(fsno, &child);
        self.invalidate_dir_cache();
        drop(parent);
        drop(context);
        evict_inode(fsno, child_ino);
        Ok(())
    }

    fn link(&self, name: &str, target: &Self) -> SysResult<()> {
        let name_bytes = name.as_bytes();
        if name_bytes.is_empty() || name_bytes.len() > u8::MAX as usize {
            return ret_errno("link: name is empty or too long", Errno::EINVAL);
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("link: context has been dropped", Errno::EIO))?;
        let target_context = target
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("link: target context has been dropped", Errno::EIO))?;
        if !Arc::ptr_eq(&context, &target_context) {
            return ret_errno("link: cross-filesystem hard link forbidden", Errno::EXDEV);
        }
        drop(target_context);

        let file_type = {
            let t = target.inode.lock();
            let mode_type = t.i_mode & S_IFMT;
            if mode_type == S_IFDIR {
                return ret_errno("link: hard link to directory forbidden", Errno::EPERM);
            }
            if mode_type != S_IFREG {
                return ret_errno("link: unsupported target inode type", Errno::EOPNOTSUPP);
            }
            EXT4_FT_REG_FILE
        };

        match self.lookup(name) {
            Ok(_) => return ret_errno("link: name already exists", Errno::EEXIST),
            Err(Errno::ENOENT) => {}
            Err(err) => return Err(err),
        }

        let context = context.lock();
        let mut parent = self.inode.lock();
        let mut child = target.inode.lock();

        ensure_dir_readable(&parent)?;
        if parent.i_flags.contains(Ext4InodeFlags::INDEX) {
            return ret_errno("link: htree indexed directory unsupported", Errno::EOPNOTSUPP);
        }

        let old_links = child.i_links_count;
        child.i_links_count = old_links
            .checked_add(1)
            .ok_or_else(|| debug_errno("link: link count overflow", Errno::EIO))?;
        let child_ino = child.ino;

        if let Err(err) = context.insert_dirent(parent.ino, &mut parent, name_bytes, child_ino, file_type) {
            child.i_links_count = old_links;
            return Err(err);
        }

        if let Err(err) = context.write_inode(&mut child) {
            child.i_links_count = old_links;
            return Err(err);
        }

        self.invalidate_dir_cache();
        Ok(())
    }

    fn rename(&self, old_name: &str, new_parent: &Self, new_name: &str) -> SysResult<()> {
        let old_name_bytes = old_name.as_bytes();
        let new_name_bytes = new_name.as_bytes();
        if old_name_bytes.is_empty()
            || old_name_bytes.len() > u8::MAX as usize
            || new_name_bytes.is_empty()
            || new_name_bytes.len() > u8::MAX as usize
        {
            return ret_errno("rename: old or new name is empty or too long", Errno::EINVAL);
        }
        if old_name == "." || old_name == ".." || new_name == "." || new_name == ".." {
            return ret_errno("rename: '.' and '..' cannot be renamed", Errno::EINVAL);
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("rename: context has been dropped", Errno::EIO))?;
        let new_parent_context = new_parent
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("rename: new parent context has been dropped", Errno::EIO))?;
        if !Arc::ptr_eq(&context, &new_parent_context) {
            return ret_errno("rename: cross-filesystem rename forbidden", Errno::EXDEV);
        }
        drop(new_parent_context);

        let context = context.lock();
        let old_parent_ino = self.get_ino();
        let new_parent_ino = new_parent.get_ino();
        let same_parent = old_parent_ino == new_parent_ino;

        if same_parent {
            let mut parent = self.inode.lock();
            ensure_dir_writable(&parent, "rename")?;

            let src_ino = lookup_name_in_dir(&context, &parent, old_name_bytes)?;
            if old_name == new_name {
                return Ok(());
            }
            match lookup_name_in_dir(&context, &parent, new_name_bytes) {
                Ok(_) => return ret_errno("rename: destination already exists", Errno::EEXIST),
                Err(Errno::ENOENT) => {}
                Err(err) => return Err(err),
            }

            let src_inode = context.read_inode(src_ino)?;
            let file_type = dirent_file_type(src_inode.i_mode)?;

            context.insert_dirent(parent.ino, &mut parent, new_name_bytes, src_ino, file_type)?;
            context.remove_dirent(parent.ino, &mut parent, old_name_bytes)?;
            self.invalidate_dir_cache();
            return Ok(());
        }

        let (mut old_parent, mut new_parent_inode) = if old_parent_ino <= new_parent_ino {
            (self.inode.lock(), new_parent.inode.lock())
        } else {
            let new_parent_inode = new_parent.inode.lock();
            let old_parent = self.inode.lock();
            (old_parent, new_parent_inode)
        };

        ensure_dir_writable(&old_parent, "rename")?;
        ensure_dir_writable(&new_parent_inode, "rename")?;

        let src_ino = lookup_name_in_dir(&context, &old_parent, old_name_bytes)?;
        match lookup_name_in_dir(&context, &new_parent_inode, new_name_bytes) {
            Ok(_) => return ret_errno("rename: destination already exists", Errno::EEXIST),
            Err(Errno::ENOENT) => {}
            Err(err) => return Err(err),
        }

        let src_inode = context.read_inode(src_ino)?;
        let src_mode_type = src_inode.i_mode & S_IFMT;
        let file_type = dirent_file_type(src_inode.i_mode)?;

        if src_mode_type == S_IFDIR && new_parent_inode.ino == src_ino {
            return ret_errno("rename: cannot move directory into itself", Errno::EINVAL);
        }

        context.insert_dirent(
            new_parent_inode.ino,
            &mut new_parent_inode,
            new_name_bytes,
            src_ino,
            file_type,
        )?;

        if src_mode_type == S_IFDIR {
            context.patch_dotdot(src_ino, &src_inode, new_parent_inode.ino)?;
            old_parent.i_links_count = old_parent
                .i_links_count
                .checked_sub(1)
                .ok_or_else(|| debug_errno("rename: old parent link count underflow", Errno::EIO))?;
            new_parent_inode.i_links_count = new_parent_inode
                .i_links_count
                .checked_add(1)
                .ok_or_else(|| debug_errno("rename: new parent link count overflow", Errno::EIO))?;
            context.write_inode(&mut old_parent)?;
            context.write_inode(&mut new_parent_inode)?;
        }

        context.remove_dirent(old_parent.ino, &mut old_parent, old_name_bytes)?;
        self.invalidate_dir_cache();
        new_parent.invalidate_dir_cache();
        if src_mode_type == S_IFDIR {
            invalidate_cached_dir(context.fsno, src_ino);
        }
        Ok(())
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("get_dent: context has been dropped", Errno::EIO))?;
        let context = context.lock();
        let inode = self.inode.lock();

        ensure_dir_readable(&inode)?;
        let entries = self.dir_results(&context, &inode)?;
        if let Some(entry) = entries.get(index) {
            return Ok(Some((entry.clone(), index + 1)));
        }
        Ok(None)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("lookup: context has been dropped", Errno::EIO))?;
        let context = context.lock();
        let inode = self.inode.lock();

        ensure_dir_readable(&inode)?;

        let needle = name.as_bytes();
        if needle.is_empty() || needle.len() > u8::MAX as usize {
            return ret_errno("lookup: name is empty or too long", Errno::ENOENT);
        }

        let entries = self.dir_results(&context, &inode)?;
        entries
            .into_iter()
            .find(|entry| entry.name.as_bytes() == needle)
            .map(|entry| entry.ino)
            .ok_or(Errno::ENOENT)
    }

    fn mode(&self) -> SysResult<Mode> {
        let inode = self.inode.lock();
        Ok(Mode::from_bits_truncate(inode.i_mode as u32))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(self.inode.lock().i_size)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        let inode = self.inode.lock();
        Ok((inode.i_uid as u32, inode.i_gid as u32))
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let blksize = {
            self.context
                .upgrade()
                .ok_or_else(|| debug_errno("fstat: context has been dropped", Errno::EIO))?
                .lock()
                .block_size()
        };
        let inode = self.inode.lock();
        Ok(FileStat {
            st_ino: inode.ino as u64,
            st_size: inode.i_size as i64,
            st_mode: inode.i_mode as u32,
            st_nlink: inode.i_links_count as u32,
            st_uid: inode.i_uid as u32,
            st_gid: inode.i_gid as u32,
            st_blksize: blksize as i32,
            st_blocks: inode.i_blocks as u64,
            ..FileStat::default()
        })
    }

    fn wrap_file(&self, inode: Arc<VfsInode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }
}

impl Drop for Inode {
    fn drop(&mut self) {
        let _ = self.sync();
    }
}

fn read_dir_results_from_disk(context: &Context, inode: &Ext4Inode) -> SysResult<Vec<DirResult>> {
    let block_size = context.block_size as u64;
    let total_blocks = u32::try_from(inode.i_size.div_ceil(block_size))
        .map_err(|_| debug_errno("read_dir_results_from_disk: logical block count overflow", Errno::EFBIG))?;
    let ino = inode.ino;
    let generation = inode.i_generation;
    let mut results = Vec::new();

    if inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
        let root = context.parse_extent_root(inode)?;
        for lblk in 0..total_blocks {
            let Some(pblk) = lookup_extent_lblk(context, root.clone(), ino, generation, lblk)? else {
                continue;
            };
            let dir_block = context.read_dir_block(ino, generation, pblk)?;
            for entry in &dir_block.entries {
                if entry.inode != 0 {
                    results.push(to_dir_result(entry));
                }
            }
        }
    } else {
        for lblk in 0..total_blocks {
            let Some(pblk) = lookup_lblk(context, inode, lblk)? else {
                continue;
            };
            let dir_block = context.read_dir_block(ino, generation, pblk)?;
            for entry in &dir_block.entries {
                if entry.inode != 0 {
                    results.push(to_dir_result(entry));
                }
            }
        }
    }

    Ok(results)
}

fn cached_or_load_dir_results(context: &Context, inode: &Ext4Inode) -> SysResult<Vec<DirResult>> {
    if let Some(cached) = cached_ext4_inode(context.fsno, inode.ino) {
        return cached.dir_results(context, inode);
    }
    read_dir_results_from_disk(context, inode)
}

fn invalidate_cached_dir(fsno: u32, ino: u32) {
    if let Some(cached) = cached_ext4_inode(fsno, ino) {
        cached.invalidate_dir_cache();
    }
}

fn advance_extent_cursor(extents: &[ExtentLeaf], extent_idx: &mut usize, lblk: u32) {
    while *extent_idx < extents.len() {
        let Some(end) = extent_end_lblk(extents[*extent_idx]) else {
            *extent_idx += 1;
            continue;
        };
        if lblk < end {
            break;
        }
        *extent_idx += 1;
    }
}

fn extent_len_blocks(raw: u16) -> u32 {
    if raw <= EXT_INIT_MAX_LEN {
        raw as u32
    } else {
        (raw - EXT_INIT_MAX_LEN) as u32
    }
}

fn extent_end_lblk(extent: ExtentLeaf) -> Option<u32> {
    extent.ee_block.checked_add(extent_len_blocks(extent.ee_len_raw))
}

fn extent_contains_lblk(extent: ExtentLeaf, lblk: u32) -> bool {
    extent
        .ee_block
        .checked_add(extent_len_blocks(extent.ee_len_raw))
        .map(|end| (extent.ee_block..end).contains(&lblk))
        .unwrap_or(false)
}

fn mapped_read_pblk(extents: &[ExtentLeaf], extent_idx: usize, lblk: u32) -> Option<u64> {
    let extent = *extents.get(extent_idx)?;
    if extent.ee_len_raw > EXT_INIT_MAX_LEN || !extent_contains_lblk(extent, lblk) {
        return None;
    }
    Some(extent.ee_start + (lblk - extent.ee_block) as u64)
}

fn mapped_write_pblk(extents: &[ExtentLeaf], extent_idx: usize, lblk: u32) -> Option<u64> {
    let extent = *extents.get(extent_idx)?;
    if !extent_contains_lblk(extent, lblk) {
        return None;
    }
    Some(extent.ee_start + (lblk - extent.ee_block) as u64)
}

fn mapped_extent_for_read(extents: &[ExtentLeaf], extent_idx: usize, lblk: u32) -> Option<ExtentLeaf> {
    let extent = *extents.get(extent_idx)?;
    if extent.ee_len_raw > EXT_INIT_MAX_LEN || !extent_contains_lblk(extent, lblk) {
        return None;
    }
    Some(extent)
}

fn mapped_extent_for_write(extents: &[ExtentLeaf], extent_idx: usize, lblk: u32) -> Option<ExtentLeaf> {
    let extent = *extents.get(extent_idx)?;
    if !extent_contains_lblk(extent, lblk) {
        return None;
    }
    Some(extent)
}

fn full_block_run_len(extent: ExtentLeaf, lblk: u32, remaining_bytes: usize, block_size: usize) -> usize {
    let Some(end_lblk) = extent_end_lblk(extent) else {
        return 0;
    };
    let run_blocks = end_lblk.saturating_sub(lblk) as usize;
    (remaining_bytes / block_size).min(run_blocks)
}

fn hole_run_blocks(extents: &[ExtentLeaf], extent_idx: usize, lblk: u32) -> usize {
    extents
        .get(extent_idx)
        .map(|extent| extent.ee_block.saturating_sub(lblk) as usize)
        .unwrap_or(usize::MAX)
}

fn allocate_full_block_run(
    context: &Context,
    extents: &mut Vec<ExtentLeaf>,
    extent_idx: &mut usize,
    lblk: u32,
    desired_blocks: usize,
    newly_allocated: &mut Vec<u64>,
) -> SysResult<(usize, Vec<(u64, usize)>)> {
    if desired_blocks == 0 {
        return Ok((0, Vec::new()));
    }

    let extents_before = extents.clone();
    let extent_idx_before = *extent_idx;
    let new_allocated_start = newly_allocated.len();
    let mut allocated = Vec::with_capacity(desired_blocks);

    for block_idx in 0..desired_blocks {
        let run_lblk = lblk
            .checked_add(block_idx as u32)
            .ok_or_else(|| debug_errno("allocate_full_block_run: logical block overflow", Errno::EFBIG))?;
        let pblk = match context.alloc_block() {
            Ok(pblk) => pblk,
            Err(err) => {
                rollback_newly_allocated(context, extents, newly_allocated, &extents_before, new_allocated_start);
                *extent_idx = extent_idx_before;
                return Err(err);
            }
        };
        if let Err(err) = context.insert_extent_mapping(extents, run_lblk, pblk) {
            let _ = context.free_block(pblk);
            rollback_newly_allocated(context, extents, newly_allocated, &extents_before, new_allocated_start);
            *extent_idx = extent_idx_before;
            return Err(err);
        }
        newly_allocated.push(pblk);
        allocated.push(pblk);
    }

    *extent_idx = extent_idx_before.saturating_sub(1);
    advance_extent_cursor(extents, extent_idx, lblk);

    let mut segments = Vec::new();
    let mut seg_start = allocated[0];
    let mut seg_len = 1usize;
    for &pblk in &allocated[1..] {
        let prev = seg_start + seg_len as u64 - 1;
        if pblk == prev + 1 {
            seg_len += 1;
        } else {
            segments.push((seg_start, seg_len));
            seg_start = pblk;
            seg_len = 1;
        }
    }
    segments.push((seg_start, seg_len));

    Ok((allocated.len(), segments))
}

fn rollback_newly_allocated(
    context: &Context,
    extents: &mut Vec<ExtentLeaf>,
    newly_allocated: &mut Vec<u64>,
    extents_before: &[ExtentLeaf],
    new_allocated_start: usize,
) {
    while newly_allocated.len() > new_allocated_start {
        if let Some(pblk) = newly_allocated.pop() {
            let _ = context.free_block(pblk);
        }
    }
    *extents = extents_before.to_vec();
}

fn ensure_dir_readable(inode: &Ext4Inode) -> SysResult<()> {
    if (inode.i_mode & S_IFMT) != S_IFDIR {
        return ret_errno("ensure_dir_readable: inode is not a directory", Errno::ENOTDIR);
    }
    if inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
        return ret_errno(
            "ensure_dir_readable: inline_data directory unsupported",
            Errno::EOPNOTSUPP,
        );
    }
    Ok(())
}

fn ensure_dir_writable(inode: &Ext4Inode, op: &str) -> SysResult<()> {
    ensure_dir_readable(inode)?;
    if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
        return ret_errno(
            &alloc::format!("{op}: non-extent directory unsupported"),
            Errno::EOPNOTSUPP,
        );
    }
    if inode.i_flags.contains(Ext4InodeFlags::INDEX) {
        return ret_errno(
            &alloc::format!("{op}: htree indexed directory unsupported"),
            Errno::EOPNOTSUPP,
        );
    }
    Ok(())
}

fn lookup_name_in_dir(context: &Context, inode: &Ext4Inode, needle: &[u8]) -> SysResult<u32> {
    cached_or_load_dir_results(context, inode)?
        .into_iter()
        .find(|entry| entry.name.as_bytes() == needle)
        .map(|entry| entry.ino)
        .ok_or(Errno::ENOENT)
}

fn dirent_file_type(mode: u16) -> SysResult<u8> {
    match mode & S_IFMT {
        S_IFREG => Ok(EXT4_FT_REG_FILE),
        S_IFDIR => Ok(EXT4_FT_DIR),
        0xA000 => Ok(EXT4_FT_SYMLINK),
        0x2000 => Ok(EXT4_FT_CHRDEV),
        0x6000 => Ok(EXT4_FT_BLKDEV),
        0x1000 => Ok(EXT4_FT_FIFO),
        0xC000 => Ok(EXT4_FT_SOCK),
        _ => ret_errno("dirent_file_type: unsupported inode type", Errno::EOPNOTSUPP),
    }
}

fn ensure_unlink_cleanup_supported(inode: &Ext4Inode) -> SysResult<()> {
    if inode.i_file_acl != 0 {
        return ret_errno("unlink: external xattr block unsupported", Errno::EOPNOTSUPP);
    }

    let mode_type = inode.i_mode & S_IFMT;
    if mode_type == S_IFDIR {
        return ret_errno("unlink: target is a directory", Errno::EISDIR);
    }

    if inode.i_flags.contains(Ext4InodeFlags::EXTENTS) || inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
        return Ok(());
    }

    if inode.i_blocks != 0 {
        return ret_errno(
            "unlink: non-extent inode with allocated blocks unsupported",
            Errno::EOPNOTSUPP,
        );
    }

    Ok(())
}

fn ensure_rmdir_cleanup_supported(inode: &Ext4Inode) -> SysResult<()> {
    if inode.i_file_acl != 0 {
        return ret_errno("rmdir: external xattr block unsupported", Errno::EOPNOTSUPP);
    }
    if (inode.i_mode & S_IFMT) != S_IFDIR {
        return ret_errno("rmdir: target is not a directory", Errno::ENOTDIR);
    }
    ensure_dir_writable(inode, "rmdir")
}

fn is_dir_empty(context: &Context, inode: &Ext4Inode) -> SysResult<bool> {
    ensure_dir_readable(inode)?;
    Ok(cached_or_load_dir_results(context, inode)?
        .into_iter()
        .all(|entry| entry.name.as_bytes() == b"." || entry.name.as_bytes() == b".."))
}

fn destroy_unlinked_inode(context: &Context, inode: &mut Ext4Inode, was_dir: bool) -> SysResult<()> {
    if inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
        context.remove_extent_range(inode.ino, inode, 0, u32::MAX)?;
        inode.i_size = 0;
    } else {
        inode.i_size = 0;
        inode.i_blocks = 0;
    }

    inode.i_links_count = 0;
    context.zero_inode_record(inode.ino)?;
    context.free_inode_bit(inode.ino, was_dir)
}

fn sync_cached_inode(fsno: u32, inode: &Ext4Inode) {
    if let Some(cached) = cached_ext4_inode(fsno, inode.ino) {
        cached.refresh_cached_state(inode);
    }
}

fn mark_cached_inode_deleted(fsno: u32, inode: &Ext4Inode) {
    if let Some(cached) = cached_ext4_inode(fsno, inode.ino) {
        cached.mark_deleted(inode);
    }
}

fn cached_ext4_inode(fsno: u32, ino: u32) -> Option<Arc<VfsInodeWrapper<Inode>>> {
    find_cached_inode(fsno, ino)?
        .downcast_arc::<VfsInodeWrapper<Inode>>()
        .ok()
}

fn to_dir_result(entry: &DirEntry2) -> DirResult {
    DirResult {
        ino: entry.inode,
        name: String::from_utf8_lossy(entry.name_slice()).into_owned(),
        file_type: ext4_file_type(entry.file_type),
    }
}

fn ext4_file_type(ft: u8) -> FileType {
    match ft {
        EXT4_FT_REG_FILE => FileType::Regular,
        EXT4_FT_DIR => FileType::Directory,
        EXT4_FT_CHRDEV => FileType::CharDevice,
        EXT4_FT_BLKDEV => FileType::BlockDevice,
        EXT4_FT_FIFO => FileType::FIFO,
        EXT4_FT_SOCK => FileType::Socket,
        EXT4_FT_SYMLINK => FileType::Symlink,
        _ => FileType::Unknown,
    }
}
