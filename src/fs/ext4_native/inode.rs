use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;

use crate::arch;
use crate::driver::chosen::kclock;
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::FileType;
use crate::fs::vfs::{evict_inode, find_cached_inode};
use crate::fs::{Dentry, Inode as VfsInode, InodeOps, Mode, Owner, VfsInode as VfsInodeWrapper};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::swappable::{FileBackend, FileMapping, FilePageIdentityPin, SwappableFramePin};
use crate::kernel::mm::{FixedContiguousPhysPageFrame, PhysPageFrame, page};
use crate::kernel::uapi::{FileStat, Uid};
use crate::klib::{SleepLock, SleepRwLockOnStack, SpinLock};

use super::ondisk::{
    DirEntry2, Ext4DirEntryFileType, Ext4InodeFlags, debug_errno, lookup_extent_lblk, lookup_lblk, ret_errno,
};
use super::{Context, Ext4Inode, ExtentLeaf};

const S_IFMT: u16 = 0xF000;
const S_IFDIR: u16 = 0x4000;
const S_IFREG: u16 = 0x8000;
const EXT_INIT_MAX_LEN: u16 = 32768;
const MAX_BULK_ALLOC_BLOCKS: usize = 128;
const WRITEBACK_BATCH_PAGES: usize = 4;

fn now() -> Duration {
    kclock::now().unwrap_or(Duration::ZERO)
}

struct InodePageCache {
    mapping: Option<Arc<FileMapping>>,
}

impl InodePageCache {
    fn new() -> Self {
        Self { mapping: None }
    }

    fn attach(&mut self, mapping: Arc<FileMapping>) {
        assert!(
            self.mapping.replace(mapping).is_none(),
            "file mapping is already attached"
        );
    }

    fn mapping(&self) -> &Arc<FileMapping> {
        self.mapping.as_ref().expect("file mapping is not attached")
    }

    fn get_page(&self, page_index: usize) -> Option<FilePageIdentityPin> {
        self.mapping().cached_page(page_index)
    }

    fn acquire_cached_page(
        &self,
        page_index: usize,
        load: impl FnOnce() -> SysResult<Option<PhysPageFrame>>,
    ) -> SysResult<FilePageIdentityPin> {
        let page = self
            .mapping()
            .acquire_cached_page(page_index, load)?
            .ok_or(Errno::EIO)?;
        Ok(page)
    }

    fn insert_frame(&self, page_index: usize, frame: PhysPageFrame) -> SysResult<FilePageIdentityPin> {
        self.acquire_cached_page(page_index, || Ok(Some(frame)))
    }

    fn insert_frame_pinned(
        &self,
        page_index: usize,
        frame: PhysPageFrame,
    ) -> SysResult<(FilePageIdentityPin, SwappableFramePin<FileBackend>)> {
        self.mapping()
            .acquire_cached_page_pinned(page_index, || Ok(Some(frame)))?
            .ok_or(Errno::EIO)
    }

    fn dirty_pages(&self) -> Vec<(usize, FilePageIdentityPin)> {
        let mut cpu_mask = 0;
        let dirty_pages = self
            .mapping()
            .pages_snapshot()
            .into_iter()
            .filter_map(|(page_index, page)| {
                let (dirty, page_cpu_mask) = page.collect_mapped_access_dirty_no_flush();
                cpu_mask |= page_cpu_mask;
                dirty.then_some((page_index, page))
            })
            .collect();
        if cpu_mask != 0 {
            arch::flush_tlb_cpu_mask(cpu_mask);
        }
        dirty_pages
    }

    fn discard_after_truncate(&mut self, new_size: usize) {
        let new_page_count = new_size.div_ceil(arch::PGSIZE);
        self.mapping().invalidate_after(new_page_count);

        let tail_offset = new_size % arch::PGSIZE;
        if tail_offset != 0
            && let Some(page) = self.get_page(new_page_count - 1)
        {
            page.with_resident_and_record_ad(true, |frame| {
                frame.slice()[tail_offset..].fill(0);
                ((), Default::default())
            });
        }
    }

    fn clear(&mut self) {
        if let Some(mapping) = &self.mapping {
            mapping.invalidate_after(0);
        }
    }
}

pub struct Inode {
    context: Weak<SleepRwLockOnStack<Context>>,
    inode: SleepLock<Ext4Inode>,
    page_cache: SleepLock<InodePageCache>,
    dents_cache: SpinLock<Option<Vec<DirResult>>>,
    metadata_dirty: AtomicBool,
    deleted: AtomicBool,
}

impl Inode {
    pub fn new(context: Weak<SleepRwLockOnStack<Context>>, inode: Ext4Inode) -> Self {
        Self {
            context,
            inode: SleepLock::new(inode, "ext4_native::Inode::inode"),
            page_cache: SleepLock::new(InodePageCache::new(), "ext4_native::Inode::page_cache"),
            dents_cache: SpinLock::new(None, "ext4_native::Inode::dents_cache"),
            metadata_dirty: AtomicBool::new(false),
            deleted: AtomicBool::new(false),
        }
    }

    fn refresh_cached_state(&self, inode: &Ext4Inode) {
        let mut current = self.inode.lock();
        let mut refreshed = inode.clone();
        if self.metadata_dirty.load(Ordering::Acquire) {
            refreshed.i_atime = current.i_atime;
            refreshed.i_mtime = current.i_mtime;
            refreshed.i_ctime = current.i_ctime;
        }
        *current = refreshed;
    }

    fn invalidate_dir_cache(&self) {
        *self.dents_cache.lock() = None;
    }

    fn dir_results(&self, context: &Context, inode: &Ext4Inode) -> SysResult<Vec<DirResult>> {
        if let Some(entries) = self.dents_cache.lock().as_ref() {
            return Ok(entries.clone());
        }

        let entries = read_dir_results_from_disk(context, inode)?;
        *self.dents_cache.lock() = Some(entries.clone());
        Ok(entries)
    }

    fn dir_result_at(&self, context: &Context, inode: &Ext4Inode, index: usize) -> SysResult<Option<DirResult>> {
        if let Some(entries) = self.dents_cache.lock().as_ref() {
            return Ok(entries.get(index).cloned());
        }

        let entries = read_dir_results_from_disk(context, inode)?;
        let result = entries.get(index).cloned();
        *self.dents_cache.lock() = Some(entries);
        Ok(result)
    }

    fn lookup_dir_entry(&self, context: &Context, inode: &Ext4Inode, needle: &[u8]) -> SysResult<u32> {
        if let Some(entries) = self.dents_cache.lock().as_ref() {
            return entries
                .iter()
                .find(|entry| entry.name.as_bytes() == needle)
                .map(|entry| entry.ino)
                .ok_or(Errno::ENOENT);
        }

        let entries = read_dir_results_from_disk(context, inode)?;
        let result = entries
            .iter()
            .find(|entry| entry.name.as_bytes() == needle)
            .map(|entry| entry.ino)
            .ok_or(Errno::ENOENT);
        *self.dents_cache.lock() = Some(entries);
        result
    }

    fn mark_deleted(&self, inode: &Ext4Inode) {
        self.metadata_dirty.store(false, Ordering::Release);
        self.refresh_cached_state(inode);
        self.invalidate_dir_cache();
        self.deleted.store(true, Ordering::Release);
    }

    fn update_metadata(&self, update: impl FnOnce(&mut Ext4Inode)) -> SysResult<()> {
        let mut inode = self.inode.lock();
        update(&mut inode);
        self.metadata_dirty.store(true, Ordering::Release);
        Ok(())
    }

    fn is_cacheable_file(inode: &Ext4Inode) -> bool {
        inode.i_mode & S_IFMT == S_IFREG && !inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA)
    }

    fn ensure_extent_data_supported(inode: &Ext4Inode, op: &str) -> SysResult<()> {
        if inode.i_flags.contains(Ext4InodeFlags::INLINE_DATA) {
            return ret_errno(
                &alloc::format!("{op}: inline_data inode unsupported"),
                Errno::EOPNOTSUPP,
            );
        }
        if !inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            return ret_errno(&alloc::format!("{op}: non-extent inode unsupported"), Errno::EOPNOTSUPP);
        }
        Ok(())
    }

    fn validate_regular_write(inode: &Ext4Inode, offset: usize, len: usize) -> SysResult<usize> {
        let mode_type = inode.i_mode & S_IFMT;
        if mode_type == S_IFDIR {
            return ret_errno("writeat: inode is a directory", Errno::EISDIR);
        }
        if mode_type != S_IFREG {
            return ret_errno("writeat: inode is not a regular file", Errno::EINVAL);
        }
        Self::ensure_extent_data_supported(inode, "writeat")?;

        offset
            .checked_add(len)
            .ok_or_else(|| debug_errno("writeat: offset+len overflow", Errno::EINVAL))
    }

    fn read_raw_at_locked(context: &Context, inode: &Ext4Inode, buf: &mut [u8], offset: usize) -> SysResult<usize> {
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

                match lookup_lblk(context, inode, lblk)? {
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
        let (_, extents, _) = context.extent_tree_snapshot(ino, inode)?;

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

    fn write_raw_at_locked(context: &Context, inode: &mut Ext4Inode, buf: &[u8], offset: usize) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let end = Self::validate_regular_write(inode, offset, buf.len())?;
        Self::write_extent_data_locked(context, inode, buf, offset, end, "writeat")
    }

    fn write_extent_data_locked(
        context: &Context,
        inode: &mut Ext4Inode,
        buf: &[u8],
        offset: usize,
        end: usize,
        op: &str,
    ) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        Self::ensure_extent_data_supported(inode, op)?;
        let block_size = context.block_size as usize;
        let ino = inode.ino;
        let (tree_generation, mut extents, old_extent_blocks) = context.extent_tree_snapshot(ino, inode)?;
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
                                context,
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
            if let Err(err) = context.replace_extent_tree(ino, inode, tree_generation, &extents, &old_extent_blocks) {
                for pblk in newly_allocated {
                    let _ = context.free_block(pblk);
                }
                return Err(err);
            }
            context.update_i_blocks_from_extent_delta(inode, &old_extents, old_extent_blocks.len(), &extents)?;
            metadata_dirty = true;
        }
        if metadata_dirty {
            context.write_inode(inode)?;
        }

        Ok(written)
    }

    fn load_page_to_cache(
        &self,
        context: &Arc<SleepRwLockOnStack<Context>>,
        page_cache: &InodePageCache,
        file_page_index: usize,
        file_size: usize,
    ) -> SysResult<FilePageIdentityPin> {
        if let Some(page) = page_cache.get_page(file_page_index) {
            return Ok(page);
        }

        let page_offset = file_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        if page_offset >= file_size {
            return Err(Errno::EINVAL);
        }

        page_cache.acquire_cached_page(file_page_index, || {
            let context = context.read();
            let inode = self.inode.lock();
            let len = core::cmp::min(file_size - page_offset, arch::PGSIZE);
            let frame = PhysPageFrame::alloc_zeroed();
            let read_len = Self::read_raw_at_locked(&context, &inode, &mut frame.slice()[..len], page_offset)?;
            if read_len < len {
                frame.slice()[read_len..len].fill(0);
            }
            Ok(Some(frame))
        })
    }

    fn flush_dirty_pages(&self) -> SysResult<()> {
        let Some(context) = self.context.upgrade() else {
            return Err(Errno::EIO);
        };
        let inode = self.inode.lock();
        if !Self::is_cacheable_file(&inode) {
            drop(inode);
            self.page_cache.lock().clear();
            return Ok(());
        }

        let file_size = usize::try_from(inode.i_size).map_err(|_| Errno::EFBIG)?;
        drop(inode);
        let page_cache = self.page_cache.lock();
        let dirty_pages = page_cache.dirty_pages();
        if dirty_pages.is_empty() {
            return Ok(());
        }
        let write_buffer = page::try_alloc_contiguous(WRITEBACK_BATCH_PAGES)
            .map(FixedContiguousPhysPageFrame::<WRITEBACK_BATCH_PAGES>::new);
        if let Some(write_buffer) = write_buffer {
            let mut index = 0;
            let mut batch = Vec::with_capacity(WRITEBACK_BATCH_PAGES);
            while index < dirty_pages.len() {
                let first_page_index = dirty_pages[index].0;
                let batch_offset = first_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
                let mut batch_len = 0;
                batch.clear();

                while index < dirty_pages.len() && batch.len() < WRITEBACK_BATCH_PAGES {
                    let (page_index, page) = &dirty_pages[index];
                    let expected_page_index = first_page_index.checked_add(batch.len()).ok_or(Errno::EFBIG)?;
                    if *page_index != expected_page_index {
                        break;
                    }

                    let offset = page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
                    let Some(mut guard) = page.try_get_page() else {
                        if batch.is_empty() {
                            index += 1;
                        }
                        break;
                    };
                    if !guard.take_dirty() || offset >= file_size {
                        if batch.is_empty() {
                            index += 1;
                        }
                        break;
                    }

                    let len = core::cmp::min(file_size - offset, arch::PGSIZE);
                    batch_len += len;
                    batch.push((guard, len));
                    index += 1;
                }

                if batch.is_empty() {
                    continue;
                }

                let write_result = if batch.len() == 1 {
                    let context = context.write();
                    let mut inode = self.inode.lock();
                    let (guard, len) = &batch[0];
                    Self::write_raw_at_locked(&context, &mut inode, &guard.frame().slice()[..*len], batch_offset)
                } else {
                    let mut buffer_offset = 0;
                    for (guard, len) in &batch {
                        write_buffer.slice()[buffer_offset..buffer_offset + len]
                            .copy_from_slice(&guard.frame().slice()[..*len]);
                        buffer_offset += len;
                    }
                    let context = context.write();
                    let mut inode = self.inode.lock();
                    Self::write_raw_at_locked(&context, &mut inode, &write_buffer.slice()[..batch_len], batch_offset)
                };
                match write_result {
                    Ok(written) if written == batch_len => {}
                    Ok(_) => {
                        for (guard, _) in &mut batch {
                            guard.mark_dirty();
                        }
                        return Err(Errno::EIO);
                    }
                    Err(err) => {
                        for (guard, _) in &mut batch {
                            guard.mark_dirty();
                        }
                        return Err(err);
                    }
                }
            }
        } else {
            for (page_index, page) in dirty_pages {
                let offset = page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
                let Some(mut guard) = page.try_get_page() else {
                    continue;
                };
                if !guard.take_dirty() || offset >= file_size {
                    continue;
                }

                let len = core::cmp::min(file_size - offset, arch::PGSIZE);
                let write_result = {
                    let context = context.write();
                    let mut inode = self.inode.lock();
                    Self::write_raw_at_locked(&context, &mut inode, &guard.frame().slice()[..len], offset)
                };
                match write_result {
                    Ok(written) if written == len => {}
                    Ok(_) => {
                        guard.mark_dirty();
                        return Err(Errno::EIO);
                    }
                    Err(err) => {
                        guard.mark_dirty();
                        return Err(err);
                    }
                }
            }
        }
        Ok(())
    }

    fn create_child(&self, name: &str, mode: Mode, owner: Owner, dev: u64, op: &str) -> SysResult<Self> {
        let file_type = dirent_file_type(mode.bits() as u16)?;
        let is_dir = file_type == Ext4DirEntryFileType::Directory;
        let name_bytes = name.as_bytes();
        if name_bytes.is_empty() || name_bytes.len() > u8::MAX as usize {
            return ret_errno(&alloc::format!("{op}: name is empty or too long"), Errno::EINVAL);
        }

        match self.lookup(name) {
            Ok(_) => return ret_errno(&alloc::format!("{op}: name already exists"), Errno::EEXIST),
            Err(Errno::ENOENT) => {}
            Err(err) => return Err(err),
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("create_child: context has been dropped", Errno::EIO))?;
        let context_lock = context.write();
        let mut parent = self.inode.lock();

        ensure_dir_writable(&parent, op)?;

        let child_ino = context_lock.alloc_inode(is_dir)?;
        let old_parent_links = parent.i_links_count;
        let mut child_block = None;
        let result = (|| -> SysResult<Self> {
            let mut child = context_lock.new_inode(
                child_ino,
                mode.bits() as u16,
                owner.uid as u16,
                owner.gid as u16,
                if is_dir { 2 } else { 1 },
            )?;
            let time = now();
            child.set_atime(&time);
            child.set_mtime(&time);
            child.set_ctime(&time);

            match file_type {
                Ext4DirEntryFileType::Directory => {
                    let pblk = context_lock.alloc_block()?;
                    child_block = Some(pblk);
                    // Zero before publishing the extent so a crash between
                    // insert_extent_1blk and init_dir_block cannot expose
                    // stale data from a previously freed block.
                    context_lock.zero_fs_block(pblk)?;
                    context_lock.insert_extent_1blk(child_ino, &mut child, 0, pblk)?;
                    context_lock.init_dir_block(pblk, child_ino, child.i_generation, parent.ino)?;
                    child.i_size = context_lock.block_size as u64;
                    parent.i_links_count = parent
                        .i_links_count
                        .checked_add(1)
                        .ok_or_else(|| debug_errno("create_child: parent link count overflow", Errno::EIO))?;
                }
                Ext4DirEntryFileType::CharacterDevice | Ext4DirEntryFileType::BlockDevice => {
                    set_device(&mut child, dev);
                }
                Ext4DirEntryFileType::Regular
                | Ext4DirEntryFileType::Fifo
                | Ext4DirEntryFileType::Socket
                | Ext4DirEntryFileType::Symlink => {}
            }

            context_lock.write_inode(&mut child)?;
            parent.set_mtime(&time);
            parent.set_ctime(&time);
            context_lock.insert_dirent(parent.ino, &mut parent, name_bytes, child_ino, file_type)?;

            Ok(Self::new(Arc::downgrade(&context), child))
        })();

        match result {
            Ok(inode) => {
                self.invalidate_dir_cache();
                Ok(inode)
            }
            Err(err) => {
                parent.i_links_count = old_parent_links;
                if let Some(pblk) = child_block {
                    let _ = context_lock.free_block(pblk);
                }
                let _ = context_lock.zero_inode_record(child_ino);
                let _ = context_lock.free_inode_bit(child_ino, is_dir);
                Err(err)
            }
        }
    }

    fn set_symlink_target(&self, target: &str) -> SysResult<()> {
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("set_symlink_target: context has been dropped", Errno::EIO))?;
        let context = context.write();
        let mut inode = self.inode.lock();

        if (inode.i_mode & S_IFMT) != Mode::S_IFLNK.bits() as u16 {
            return Err(Errno::EINVAL);
        }

        let target = target.as_bytes();
        if target.len() <= inode.i_block().len() {
            if inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
                context.remove_extent_range(inode.ino, &mut inode, 0, u32::MAX)?;
            }
            set_fast_symlink(&mut inode, target);
            let time = now();
            inode.set_mtime(&time);
            inode.set_ctime(&time);
            context.write_inode(&mut inode)?;
            return Ok(());
        }

        if inode.i_flags.contains(Ext4InodeFlags::EXTENTS) {
            context.remove_extent_range(inode.ino, &mut inode, 0, u32::MAX)?;
        } else {
            inode.i_flags.insert(Ext4InodeFlags::EXTENTS);
            context.init_empty_extent_root(&mut inode)?;
        }
        inode.i_size = 0;
        inode.i_blocks = 0;

        let written = Self::write_extent_data_locked(&context, &mut inode, target, 0, target.len(), "symlink")?;
        if written != target.len() {
            return Err(Errno::EIO);
        }

        let time = now();
        inode.set_mtime(&time);
        inode.set_ctime(&time);
        context.write_inode(&mut inode)
    }
}

impl InodeOps for Inode {
    fn attach_file_mapping(&mut self, mapping: Arc<FileMapping>) {
        self.page_cache.lock().attach(mapping);
    }

    fn get_ino(&self) -> u32 {
        self.inode.lock().ino
    }

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.deleted.load(Ordering::Acquire) {
            return ret_errno("readat: inode has been deleted", Errno::EIO);
        }
        if direct {
            self.flush_dirty_pages()?;
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("readat: context has been dropped", Errno::EIO))?;
        let file_size = {
            let context = context.read();
            let inode = self.inode.lock();
            if direct || !Self::is_cacheable_file(&inode) {
                return Self::read_raw_at_locked(&context, &inode, buf, offset);
            }
            usize::try_from(inode.i_size).map_err(|_| Errno::EFBIG)?
        };
        if offset >= file_size {
            return Ok(0);
        }

        let to_read = buf.len().min(file_size - offset);
        let page_cache = self.page_cache.lock();
        let mut read_len = 0;
        while read_len < to_read {
            let current_offset = offset + read_len;
            let page_index = current_offset / arch::PGSIZE;
            let page_offset = current_offset % arch::PGSIZE;
            let copy_len = core::cmp::min(to_read - read_len, arch::PGSIZE - page_offset);

            let page = if let Some(page) = page_cache.get_page(page_index) {
                page
            } else {
                self.load_page_to_cache(&context, &page_cache, page_index, file_size)?
            };
            let guard = page
                .ensure_page()
                .expect("ext4 cached read page must have valid backing");
            guard
                .frame()
                .copy_to_slice(page_offset, &mut buf[read_len..read_len + copy_len]);

            read_len += copy_len;
        }

        Ok(read_len)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("writeat: context has been dropped", Errno::EIO))?;
        if self.deleted.load(Ordering::Acquire) {
            return ret_errno("writeat: inode has been deleted", Errno::EIO);
        }
        let page_cache = self.page_cache.lock();
        let (end, old_size) = {
            let inode = self.inode.lock();
            if !Self::is_cacheable_file(&inode) {
                drop(inode);
                let context = context.write();
                let mut inode = self.inode.lock();
                return Self::write_raw_at_locked(&context, &mut inode, buf, offset);
            }
            (
                Self::validate_regular_write(&inode, offset, buf.len())?,
                usize::try_from(inode.i_size).map_err(|_| Errno::EFBIG)?,
            )
        };
        let mut written = 0;
        while written < buf.len() {
            let current_offset = offset + written;
            let page_index = current_offset / arch::PGSIZE;
            let page_offset = current_offset % arch::PGSIZE;
            let copy_len = core::cmp::min(buf.len() - written, arch::PGSIZE - page_offset);
            let page_start = page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
            let new_size = current_offset + copy_len;
            let extends_size = new_size > old_size;

            let (page, resident_pin) = if let Some(page) = page_cache.get_page(page_index) {
                let resident_pin = extends_size.then(|| {
                    page.pin_page(false)
                        .expect("ext4 cached write page must have valid backing")
                });
                (page, resident_pin)
            } else if page_start < old_size && copy_len != arch::PGSIZE {
                let page = self.load_page_to_cache(&context, &page_cache, page_index, old_size)?;
                let resident_pin = extends_size.then(|| {
                    page.pin_page(false)
                        .expect("ext4 cached write page must have valid backing")
                });
                (page, resident_pin)
            } else if extends_size {
                let (page, resident_pin) = page_cache.insert_frame_pinned(page_index, PhysPageFrame::alloc_zeroed())?;
                (page, Some(resident_pin))
            } else {
                (
                    page_cache.insert_frame(page_index, PhysPageFrame::alloc_zeroed())?,
                    None,
                )
            };
            let mut guard = page
                .ensure_page()
                .expect("ext4 cached write page must have valid backing");
            guard
                .frame()
                .copy_from_slice(page_offset, &buf[written..written + copy_len]);
            guard.mark_dirty();
            drop(guard);

            // Reclaim writeback bounds the page by i_size. Publish the copied
            // range before releasing its resident pin so extension data cannot
            // be skipped and then evicted as if it were still beyond EOF.
            if let Some(resident_pin) = resident_pin {
                let mut inode = self.inode.lock();
                inode.i_size = inode.i_size.max(new_size as u64);
                drop(inode);
                drop(resident_pin);
            }

            written += copy_len;
        }

        if end > old_size {
            self.metadata_dirty.store(true, Ordering::Release);
        }
        drop(page_cache);

        Ok(written)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        if self.deleted.load(Ordering::Acquire) {
            return ret_errno("truncate: inode has been deleted", Errno::EIO);
        }
        self.flush_dirty_pages()?;

        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("truncate: context has been dropped", Errno::EIO))?;
        let context = context.write();
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
        let new_size = usize::try_from(new_size).map_err(|_| Errno::EFBIG)?;
        drop(inode);
        drop(context);
        self.page_cache.lock().discard_after_truncate(new_size);
        Ok(())
    }

    fn sync(&self) -> SysResult<()> {
        if self.deleted.load(Ordering::Acquire) {
            return Ok(());
        }
        self.flush_dirty_pages()?;
        if !self.metadata_dirty.swap(false, Ordering::AcqRel) {
            return Ok(());
        }

        let Some(context) = self.context.upgrade() else {
            self.metadata_dirty.store(true, Ordering::Release);
            return Err(Errno::EIO);
        };
        let context = context.write();
        let mut inode = self.inode.lock();
        if let Err(err) = context.write_inode(&mut inode) {
            self.metadata_dirty.store(true, Ordering::Release);
            return Err(err);
        }
        Ok(())
    }

    fn type_name(&self) -> &'static str {
        "ext4_native"
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Self> {
        match mode & Mode::S_IFMT {
            Mode::S_IFREG | Mode::S_IFDIR | Mode::S_IFLNK => self.create_child(name, mode, owner, 0, "create"),
            _ => ret_errno("create: unsupported inode type", Errno::EOPNOTSUPP),
        }
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Self> {
        match mode & Mode::S_IFMT {
            Mode::S_IFCHR | Mode::S_IFBLK => self.create_child(name, mode, owner, dev, "mknod"),
            Mode::S_IFIFO | Mode::S_IFSOCK => self.create_child(name, mode, owner, 0, "mknod"),
            _ => Err(Errno::EOPNOTSUPP),
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
        let context = context.write();
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
        let context = context.write();
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
            dirent_file_type(t.i_mode)?
        };

        match self.lookup(name) {
            Ok(_) => return ret_errno("link: name already exists", Errno::EEXIST),
            Err(Errno::ENOENT) => {}
            Err(err) => return Err(err),
        }

        let context = context.write();
        let mut parent = self.inode.lock();
        let mut child = target.inode.lock();

        ensure_dir_readable(&parent)?;
        if parent.i_flags.contains(Ext4InodeFlags::INDEX) {
            return ret_errno("link: htree indexed directory unsupported", Errno::EOPNOTSUPP);
        }

        let old_links = child.i_links_count;
        let old_ctime = child.i_ctime;
        child.i_links_count = old_links
            .checked_add(1)
            .ok_or_else(|| debug_errno("link: link count overflow", Errno::EIO))?;
        let time = now();
        child.set_ctime(&time);
        let child_ino = child.ino;
        parent.set_mtime(&time);
        parent.set_ctime(&time);

        if let Err(err) = context.insert_dirent(parent.ino, &mut parent, name_bytes, child_ino, file_type) {
            child.i_links_count = old_links;
            child.i_ctime = old_ctime;
            return Err(err);
        }

        if let Err(err) = context.write_inode(&mut child) {
            child.i_links_count = old_links;
            child.i_ctime = old_ctime;
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

        let context = context.write();
        let old_parent_ino = self.get_ino();
        let new_parent_ino = new_parent.get_ino();
        let same_parent = old_parent_ino == new_parent_ino;

        if same_parent {
            let fsno;
            let mut target_to_evict = None;
            let mut parent = self.inode.lock();
            ensure_dir_writable(&parent, "rename")?;

            let src_ino = lookup_name_in_dir(&context, &parent, old_name_bytes)?;
            if old_name == new_name {
                return Ok(());
            }
            let mut src_inode = context.read_inode(src_ino)?;
            let src_mode_type = src_inode.i_mode & S_IFMT;
            let file_type = dirent_file_type(src_inode.i_mode)?;
            fsno = context.fsno;

            match lookup_name_in_dir(&context, &parent, new_name_bytes) {
                Ok(target_ino) if target_ino == src_ino => return Ok(()),
                Ok(target_ino) => {
                    target_to_evict =
                        remove_rename_target(&context, &mut parent, new_name_bytes, src_mode_type, target_ino)?
                }
                Err(Errno::ENOENT) => {}
                Err(err) => return Err(err),
            }

            let time = now();
            parent.set_mtime(&time);
            parent.set_ctime(&time);
            src_inode.set_ctime(&time);

            context.insert_dirent(parent.ino, &mut parent, new_name_bytes, src_ino, file_type)?;
            context.remove_dirent(parent.ino, &mut parent, old_name_bytes)?;
            context.write_inode(&mut parent)?;
            context.write_inode(&mut src_inode)?;
            sync_cached_inode(fsno, &src_inode);
            self.invalidate_dir_cache();
            drop(parent);
            drop(context);
            if let Some(ino) = target_to_evict {
                evict_inode(fsno, ino);
            }
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
        let mut src_inode = context.read_inode(src_ino)?;
        let src_mode_type = src_inode.i_mode & S_IFMT;
        let file_type = dirent_file_type(src_inode.i_mode)?;

        if src_mode_type == S_IFDIR && new_parent_inode.ino == src_ino {
            return ret_errno("rename: cannot move directory into itself", Errno::EINVAL);
        }

        let fsno = context.fsno;
        let mut target_to_evict = None;
        match lookup_name_in_dir(&context, &new_parent_inode, new_name_bytes) {
            Ok(target_ino) if target_ino == src_ino => return Ok(()),
            Ok(target_ino) => {
                target_to_evict = remove_rename_target(
                    &context,
                    &mut new_parent_inode,
                    new_name_bytes,
                    src_mode_type,
                    target_ino,
                )?
            }
            Err(Errno::ENOENT) => {}
            Err(err) => return Err(err),
        }

        let time = now();
        old_parent.set_mtime(&time);
        old_parent.set_ctime(&time);
        new_parent_inode.set_mtime(&time);
        new_parent_inode.set_ctime(&time);
        src_inode.set_ctime(&time);

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
        context.write_inode(&mut old_parent)?;
        context.write_inode(&mut new_parent_inode)?;
        context.write_inode(&mut src_inode)?;
        sync_cached_inode(fsno, &src_inode);
        self.invalidate_dir_cache();
        new_parent.invalidate_dir_cache();
        if src_mode_type == S_IFDIR {
            invalidate_cached_dir(context.fsno, src_ino);
        }
        drop(old_parent);
        drop(new_parent_inode);
        drop(context);
        if let Some(ino) = target_to_evict {
            evict_inode(fsno, ino);
        }
        Ok(())
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("get_dent: context has been dropped", Errno::EIO))?;
        let context = context.read();
        let inode = self.inode.lock();

        ensure_dir_readable(&inode)?;
        Ok(self
            .dir_result_at(&context, &inode, index)?
            .map(|entry| (entry, index + 1)))
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("lookup: context has been dropped", Errno::EIO))?;
        let context = context.read();
        let inode = self.inode.lock();

        ensure_dir_readable(&inode)?;

        let needle = name.as_bytes();
        if needle.is_empty() || needle.len() > u8::MAX as usize {
            return ret_errno("lookup: name is empty or too long", Errno::ENOENT);
        }

        self.lookup_dir_entry(&context, &inode, needle)
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        self.set_symlink_target(target)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("readlink: context has been dropped", Errno::EIO))?;
        let context = context.read();
        let inode = self.inode.lock();

        if (inode.i_mode & S_IFMT) != Mode::S_IFLNK.bits() as u16 {
            return Ok(None);
        }

        let len = core::cmp::min(buf.len(), usize::try_from(inode.i_size).map_err(|_| Errno::EFBIG)?);
        if inode.i_blocks == 0 && inode.i_size <= inode.i_block().len() as u64 {
            buf[..len].copy_from_slice(&inode.i_block()[..len]);
            return Ok(Some(len));
        }

        let read_len = Self::read_raw_at_locked(&context, &inode, buf, 0)?;
        Ok(Some(read_len))
    }

    fn mode(&self) -> SysResult<Mode> {
        let inode = self.inode.lock();
        Ok(Mode::from_bits_truncate(inode.i_mode as u32))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(self.inode.lock().i_size)
    }

    fn mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        self.load_raw_page(file_page_index).map(|frame| frame.map(Arc::new))
    }

    fn load_raw_page(&self, file_page_index: usize) -> SysResult<Option<PhysPageFrame>> {
        let offset = file_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("mmap_shared_page: context has been dropped", Errno::EIO))?;
        let frame = PhysPageFrame::alloc_with_shrink_zeroed();
        let context = context.read();
        if self.deleted.load(Ordering::Acquire) {
            return ret_errno("load_raw_page: inode has been deleted", Errno::EIO);
        }
        let inode = self.inode.lock();

        let file_size = usize::try_from(inode.i_size).map_err(|_| Errno::EFBIG)?;
        if offset >= file_size {
            return Ok(None);
        }

        let len = core::cmp::min(file_size - offset, arch::PGSIZE);
        let read_len = Self::read_raw_at_locked(&context, &inode, &mut frame.slice()[..len], offset)?;
        if read_len < len {
            frame.slice()[read_len..len].fill(0);
        }
        Ok(Some(frame))
    }

    fn supports_file_mapping(&self) -> bool {
        Self::is_cacheable_file(&self.inode.lock())
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        let offset = file_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let context = self
            .context
            .upgrade()
            .ok_or_else(|| debug_errno("writeback_mmap_shared_page: context has been dropped", Errno::EIO))?;
        let context = context.write();
        if self.deleted.load(Ordering::Acquire) {
            return ret_errno("writeback_mmap_shared_page: inode has been deleted", Errno::EIO);
        }
        let mut inode = self.inode.lock();

        let file_size = usize::try_from(inode.i_size).map_err(|_| Errno::EFBIG)?;
        if offset >= file_size {
            return Ok(());
        }

        let len = core::cmp::min(file_size - offset, arch::PGSIZE);
        let written = Self::write_raw_at_locked(&context, &mut inode, &frame.slice()[..len], offset)?;
        if written != len {
            return Err(Errno::EIO);
        }

        Ok(())
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        let inode = self.inode.lock();
        Ok((inode.i_uid as u32, inode.i_gid as u32))
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        self.update_metadata(|inode| {
            let current = inode.i_mode as u32;
            inode.i_mode = ((current & !0o7777) | (mode.bits() & 0o7777)) as u16;
            inode.set_ctime(&now());
        })
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        self.update_metadata(|inode| {
            if let Some(uid) = uid {
                inode.i_uid = uid as u16;
            }
            if let Some(gid) = gid {
                inode.i_gid = gid as u16;
            }

            let mut mode = inode.i_mode as u32;
            if (mode & Mode::S_IFMT.bits()) != Mode::S_IFDIR.bits() {
                if mode & Mode::S_IXGRP.bits() != 0 {
                    mode &= !(Mode::S_ISUID | Mode::S_ISGID).bits();
                } else {
                    mode &= !Mode::S_ISUID.bits();
                }
                inode.i_mode = mode as u16;
            }
            inode.set_ctime(&now());
        })
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.mode().map(|mode| mode.into())
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let blksize = {
            self.context
                .upgrade()
                .ok_or_else(|| debug_errno("fstat: context has been dropped", Errno::EIO))?
                .read()
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
            st_atime_sec: inode.i_atime as i64,
            st_mtime_sec: inode.i_mtime as i64,
            st_ctime_sec: inode.i_ctime as i64,
            ..FileStat::default()
        })
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        self.update_metadata(|inode| inode.set_atime(time))
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        self.update_metadata(|inode| inode.set_mtime(time))
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        self.update_metadata(|inode| inode.set_ctime(time))
    }

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()> {
        self.update_metadata(|inode| {
            inode.set_mtime(time);
            inode.set_ctime(time);
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
    let last_block_idx = u32::try_from(desired_blocks - 1)
        .map_err(|_| debug_errno("allocate_full_block_run: block count does not fit u32", Errno::EFBIG))?;
    lblk.checked_add(last_block_idx)
        .ok_or_else(|| debug_errno("allocate_full_block_run: logical block overflow", Errno::EFBIG))?;

    let extents_before = extents.clone();
    let extent_idx_before = *extent_idx;
    let new_allocated_start = newly_allocated.len();
    let mut allocated = Vec::with_capacity(desired_blocks);

    while allocated.len() < desired_blocks {
        let batch = match context.alloc_blocks(desired_blocks - allocated.len()) {
            Ok(batch) => batch,
            Err(err) => {
                rollback_newly_allocated(context, extents, newly_allocated, &extents_before, new_allocated_start);
                *extent_idx = extent_idx_before;
                return Err(err);
            }
        };
        let batch_start = allocated.len();
        newly_allocated.extend(batch.iter().copied());
        for (batch_idx, &pblk) in batch.iter().enumerate() {
            let block_idx = batch_start + batch_idx;
            let run_lblk = lblk + block_idx as u32;
            if let Err(err) = context.insert_extent_mapping(extents, run_lblk, pblk) {
                rollback_newly_allocated(context, extents, newly_allocated, &extents_before, new_allocated_start);
                *extent_idx = extent_idx_before;
                return Err(err);
            }
        }
        allocated.extend(batch);
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
    Ok(())
}

fn lookup_name_in_dir(context: &Context, inode: &Ext4Inode, needle: &[u8]) -> SysResult<u32> {
    if let Some(cached) = cached_ext4_inode(context.fsno, inode.ino) {
        return cached.lookup_dir_entry(context, inode, needle);
    }

    read_dir_results_from_disk(context, inode)?
        .into_iter()
        .find(|entry| entry.name.as_bytes() == needle)
        .map(|entry| entry.ino)
        .ok_or(Errno::ENOENT)
}

fn dirent_file_type(mode: u16) -> SysResult<Ext4DirEntryFileType> {
    match mode & S_IFMT {
        S_IFREG => Ok(Ext4DirEntryFileType::Regular),
        S_IFDIR => Ok(Ext4DirEntryFileType::Directory),
        0xA000 => Ok(Ext4DirEntryFileType::Symlink),
        0x2000 => Ok(Ext4DirEntryFileType::CharacterDevice),
        0x6000 => Ok(Ext4DirEntryFileType::BlockDevice),
        0x1000 => Ok(Ext4DirEntryFileType::Fifo),
        0xC000 => Ok(Ext4DirEntryFileType::Socket),
        _ => ret_errno("dirent_file_type: unsupported inode type", Errno::EOPNOTSUPP),
    }
}

fn set_device(inode: &mut Ext4Inode, dev: u64) {
    let dev = dev as u32;
    let offset = if dev & !0xffff != 0 { 4 } else { 0 };
    let block = inode.i_block_mut();
    block.fill(0);
    block[offset..offset + core::mem::size_of::<u32>()].copy_from_slice(&dev.to_le_bytes());
}

fn set_fast_symlink(inode: &mut Ext4Inode, target: &[u8]) {
    let block = inode.i_block_mut();
    block.fill(0);
    block[..target.len()].copy_from_slice(target);
    inode.i_flags.remove(Ext4InodeFlags::EXTENTS);
    inode.i_size = target.len() as u64;
    inode.i_blocks = 0;
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

fn check_rename_target_type(source_mode_type: u16, target_mode_type: u16) -> SysResult<()> {
    if source_mode_type == S_IFDIR && target_mode_type != S_IFDIR {
        return ret_errno("rename: directory cannot replace non-directory", Errno::ENOTDIR);
    }
    if source_mode_type != S_IFDIR && target_mode_type == S_IFDIR {
        return ret_errno("rename: non-directory cannot replace directory", Errno::EISDIR);
    }
    Ok(())
}

fn remove_rename_target(
    context: &Context,
    parent: &mut Ext4Inode,
    name: &[u8],
    source_mode_type: u16,
    target_ino: u32,
) -> SysResult<Option<u32>> {
    let mut target = context.read_inode(target_ino)?;
    let target_mode_type = target.i_mode & S_IFMT;
    check_rename_target_type(source_mode_type, target_mode_type)?;

    if target.i_links_count == 0 {
        return ret_errno("rename: target inode link count is already zero", Errno::EIO);
    }

    if target_mode_type == S_IFDIR {
        ensure_rmdir_cleanup_supported(&target)?;
        if !is_dir_empty(context, &target)? {
            return ret_errno("rename: target directory is not empty", Errno::ENOTEMPTY);
        }

        context.remove_dirent(parent.ino, parent, name)?;
        parent.i_links_count = parent
            .i_links_count
            .checked_sub(1)
            .ok_or_else(|| debug_errno("rename: parent link count underflow", Errno::EIO))?;
        destroy_unlinked_inode(context, &mut target, true)?;
        mark_cached_inode_deleted(context.fsno, &target);
        return Ok(Some(target_ino));
    }

    if target.i_links_count > 1 {
        let old_links = target.i_links_count;
        let old_ctime = target.i_ctime;
        target.i_links_count -= 1;
        target.set_ctime(&now());
        context.write_inode(&mut target)?;
        sync_cached_inode(context.fsno, &target);

        if let Err(err) = context.remove_dirent(parent.ino, parent, name) {
            target.i_links_count = old_links;
            target.i_ctime = old_ctime;
            let _ = context.write_inode(&mut target);
            sync_cached_inode(context.fsno, &target);
            return Err(err);
        }

        return Ok(None);
    }

    ensure_unlink_cleanup_supported(&target)?;
    context.remove_dirent(parent.ino, parent, name)?;
    destroy_unlinked_inode(context, &mut target, false)?;
    mark_cached_inode_deleted(context.fsno, &target);
    Ok(Some(target_ino))
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
    match Ext4DirEntryFileType::try_from(ft) {
        Ok(Ext4DirEntryFileType::Regular) => FileType::Regular,
        Ok(Ext4DirEntryFileType::Directory) => FileType::Directory,
        Ok(Ext4DirEntryFileType::CharacterDevice) => FileType::CharDevice,
        Ok(Ext4DirEntryFileType::BlockDevice) => FileType::BlockDevice,
        Ok(Ext4DirEntryFileType::Fifo) => FileType::FIFO,
        Ok(Ext4DirEntryFileType::Socket) => FileType::Socket,
        Ok(Ext4DirEntryFileType::Symlink) => FileType::Symlink,
        Err(_) => FileType::Unknown,
    }
}
