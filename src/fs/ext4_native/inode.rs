use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::time::Duration;

use crate::arch;
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{Inode as VfsInode, InodeLockState, InodeOps, Mode, Owner};
use crate::fs::{Dentry, FileType};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::uapi::{FileFallocateFlags, FileStat, Uid};
use crate::klib::{SleepLock, SpinLock};

use super::superblock::{InodePageCache, SuperBlockInner};

pub struct Inode {
    ino: u32,
    superblock: Arc<SleepLock<SuperBlockInner>>,
    cacheable_file: bool,
    cached_size: AtomicUsize,
    fast_cached_write: AtomicBool,
    last_write_time_sec: AtomicUsize,
    page_cache: SleepLock<InodePageCache>,
    lock_state: SpinLock<InodeLockState>,
}

impl Inode {
    const WRITEBACK_BATCH_PAGES: usize = 128;

    pub fn new(ino: u32, superblock: Arc<SleepLock<SuperBlockInner>>) -> SysResult<Self> {
        let (cacheable_file, cached_size, fast_cached_write) = superblock.lock().inode_cache_state(ino)?;
        Ok(Self {
            ino,
            superblock,
            cacheable_file,
            cached_size: AtomicUsize::new(cached_size),
            fast_cached_write: AtomicBool::new(fast_cached_write),
            last_write_time_sec: AtomicUsize::new(0),
            page_cache: SleepLock::new(InodePageCache::new(), "Ext4NativeInode::page_cache"),
            lock_state: SpinLock::new(InodeLockState::new(), "Ext4NativeInode::lock_state"),
        })
    }

    fn read_raw_at(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        self.superblock.lock().read_inode(self.ino, buf, offset, None)
    }

    fn write_raw_at(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        self.superblock.lock().write_inode(self.ino, buf, offset)
    }

    fn writeback_raw_at(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        self.superblock.lock().writeback_inode(self.ino, buf, offset)
    }

    fn prepare_cached_write(&self, offset: usize, len: usize) -> SysResult<usize> {
        let cached_size = self.cached_size.load(Ordering::Relaxed);
        self.superblock
            .lock()
            .prepare_cached_write(self.ino, offset, len, cached_size)
    }

    fn try_fast_cached_write(&self, buf: &[u8], offset: usize) -> SysResult<Option<usize>> {
        if !self.fast_cached_write.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let end = offset.checked_add(buf.len()).ok_or(Errno::EFBIG)?;
        if end > self.cached_size.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let mut page_cache = self.page_cache.lock();
        if page_cache.write_if_cached(buf, offset) {
            Ok(Some(buf.len()))
        } else {
            Ok(None)
        }
    }

    fn load_page_to_cache(
        &self,
        page_cache: &mut InodePageCache,
        page_index: usize,
        file_size: usize,
    ) -> SysResult<Arc<PhysPageFrame>> {
        if let Some(frame) = page_cache.get_frame(page_index) {
            return Ok(frame);
        }

        let page_offset = page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        if page_offset >= file_size {
            return Err(Errno::EINVAL);
        }

        let len = core::cmp::min(file_size - page_offset, arch::PGSIZE);
        let frame = Arc::new(PhysPageFrame::alloc_zeroed());
        let read_len = self.read_raw_at(&mut frame.slice()[..len], page_offset)?;
        if read_len < len {
            frame.slice()[read_len..len].fill(0);
        }

        Ok(page_cache.insert_frame(page_index, frame))
    }

    fn flush_dirty_pages(&self) -> SysResult<()> {
        if !self.cacheable_file {
            return Ok(());
        }

        let mut page_cache = self.page_cache.lock();
        let file_size = self.cached_size.load(Ordering::Relaxed);
        let dirty_pages = page_cache.dirty_pages();
        let mut index = 0;
        while index < dirty_pages.len() {
            let page_index = dirty_pages[index];
            let offset = page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
            if offset >= file_size {
                page_cache.mark_clean(page_index);
                index += 1;
                continue;
            }

            let mut run_end = index + 1;
            while run_end < dirty_pages.len() {
                let previous = dirty_pages[run_end - 1];
                let current = dirty_pages[run_end];
                if current != previous + 1 {
                    break;
                }
                let current_offset = current.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
                if current_offset >= file_size {
                    break;
                }
                run_end += 1;
            }

            if run_end == index + 1 {
                let len = core::cmp::min(file_size - offset, arch::PGSIZE);
                let Some(frame) = page_cache.get_frame(page_index) else {
                    return Err(Errno::EIO);
                };
                let written = self.writeback_raw_at(&frame.slice()[..len], offset)?;
                if written != len {
                    return Err(Errno::EIO);
                }
                page_cache.mark_clean(page_index);
            } else {
                let mut batch_start = index;
                while batch_start < run_end {
                    let batch_end = core::cmp::min(batch_start + Self::WRITEBACK_BATCH_PAGES, run_end);
                    let batch_offset = dirty_pages[batch_start].checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
                    let max_batch_size = (batch_end - batch_start)
                        .checked_mul(arch::PGSIZE)
                        .ok_or(Errno::EFBIG)?;
                    let batch_size = core::cmp::min(file_size - batch_offset, max_batch_size);
                    let mut write_buf = Vec::with_capacity(batch_size);

                    for &page_index in dirty_pages[batch_start..batch_end].iter() {
                        let page_offset = page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
                        let len = core::cmp::min(file_size - page_offset, arch::PGSIZE);
                        let Some(frame) = page_cache.get_frame(page_index) else {
                            return Err(Errno::EIO);
                        };
                        write_buf.extend_from_slice(&frame.slice()[..len]);
                    }

                    let written = self.writeback_raw_at(&write_buf, batch_offset)?;
                    if written != write_buf.len() {
                        return Err(Errno::EIO);
                    }

                    for &page_index in dirty_pages[batch_start..batch_end].iter() {
                        page_cache.mark_clean(page_index);
                    }
                    batch_start = batch_end;
                }
            }

            index = run_end;
        }
        Ok(())
    }
}

impl InodeOps for Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "ext4native"
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn begin_write_open(&self) -> SysResult<()> {
        if self.superblock.lock().is_readonly() {
            return Err(Errno::EROFS);
        }

        let mut lock_state = self.lock_state.lock();
        if lock_state.exec_count() > 0 {
            return Err(Errno::ETXTBSY);
        }
        lock_state.increment_writer_count();
        Ok(())
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        let child_ino = self.superblock.lock().create_child(self.ino, name, mode, owner, 0)?;
        self.page_cache.lock().clear();
        Ok(Arc::new(Self::new(child_ino, self.superblock.clone())?))
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Arc<dyn InodeOps>> {
        match mode & Mode::S_IFMT {
            Mode::S_IFCHR | Mode::S_IFBLK | Mode::S_IFIFO => {
                let child_ino = self.superblock.lock().create_child(self.ino, name, mode, owner, dev)?;
                self.page_cache.lock().clear();
                Ok(Arc::new(Self::new(child_ino, self.superblock.clone())?))
            }
            _ => Err(Errno::EOPNOTSUPP),
        }
    }

    fn link(&self, name: &str, target: &Arc<dyn InodeOps>) -> SysResult<()> {
        let target = target.downcast_ref::<Self>().ok_or(Errno::EXDEV)?;
        if !Arc::ptr_eq(&self.superblock, &target.superblock) {
            return Err(Errno::EXDEV);
        }
        self.superblock.lock().link_child(self.ino, name, target.ino)?;
        self.page_cache.lock().clear();
        Ok(())
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        self.superblock.lock().unlink_child(self.ino, name)?;
        self.page_cache.lock().clear();
        Ok(())
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        self.superblock.lock().set_symlink(self.ino, target)?;
        self.page_cache.lock().clear();
        Ok(())
    }

    fn rename(&self, old_name: &str, new_parent: &Arc<dyn InodeOps>, new_name: &str) -> SysResult<()> {
        let new_parent = new_parent.downcast_ref::<Self>().ok_or(Errno::EXDEV)?;
        if !Arc::ptr_eq(&self.superblock, &new_parent.superblock) {
            return Err(Errno::EXDEV);
        }
        self.superblock
            .lock()
            .rename_child(self.ino, old_name, new_parent.ino, new_name)?;
        self.page_cache.lock().clear();
        if self.ino != new_parent.ino {
            new_parent.page_cache.lock().clear();
        }
        Ok(())
    }

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if !self.cacheable_file {
            return self.read_raw_at(buf, offset);
        }

        let mut page_cache = self.page_cache.lock();
        if direct {
            let read_len = self.read_raw_at(buf, offset)?;
            page_cache.copy_to_slice(buf, offset, read_len);
            return Ok(read_len);
        }

        let file_size = self.cached_size.load(Ordering::Relaxed);
        if offset >= file_size {
            return Ok(0);
        }

        let to_read = core::cmp::min(buf.len(), file_size - offset);
        let mut read_len = 0;
        while read_len < to_read {
            let current = offset.checked_add(read_len).ok_or(Errno::EFBIG)?;
            let page_index = current / arch::PGSIZE;
            let page_offset = current % arch::PGSIZE;
            let copy_len = core::cmp::min(to_read - read_len, arch::PGSIZE - page_offset);

            let frame = self.load_page_to_cache(&mut page_cache, page_index, file_size)?;
            frame.copy_to_slice(page_offset, &mut buf[read_len..read_len + copy_len]);
            read_len += copy_len;
        }

        Ok(read_len)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if !self.cacheable_file {
            return self.write_raw_at(buf, offset);
        }
        if let Some(written) = self.try_fast_cached_write(buf, offset)? {
            return Ok(written);
        }

        let old_size = self.prepare_cached_write(offset, buf.len())?;
        let mut page_cache = self.page_cache.lock();
        let mut written = 0;
        while written < buf.len() {
            let current = offset.checked_add(written).ok_or(Errno::EFBIG)?;
            let page_index = current / arch::PGSIZE;
            let page_offset = current % arch::PGSIZE;
            let copy_len = core::cmp::min(buf.len() - written, arch::PGSIZE - page_offset);
            let page_start = page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;

            let frame = if let Some(frame) = page_cache.get_frame(page_index) {
                frame
            } else if page_start < old_size && copy_len != arch::PGSIZE {
                self.load_page_to_cache(&mut page_cache, page_index, old_size)?
            } else {
                page_cache.insert_frame(page_index, Arc::new(PhysPageFrame::alloc_zeroed()))
            };
            frame.copy_from_slice(page_offset, &buf[written..written + copy_len]);
            page_cache.mark_dirty(page_index);
            written += copy_len;
        }

        let new_size = core::cmp::max(old_size, offset.checked_add(written).ok_or(Errno::EFBIG)?);
        if new_size > old_size {
            self.cached_size.fetch_max(new_size, Ordering::Relaxed);
        }
        let should_flush = page_cache.is_over_capacity();
        drop(page_cache);

        if should_flush {
            self.flush_dirty_pages()?;
            self.page_cache.lock().shrink_to_capacity();
        }
        Ok(written)
    }

    fn mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        let offset = file_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let file_size = self.cached_size.load(Ordering::Relaxed);
        if offset >= file_size {
            return Ok(None);
        }

        let mut page_cache = self.page_cache.lock();
        self.load_page_to_cache(&mut page_cache, file_page_index, file_size)
            .map(Some)
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        let offset = file_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let file_size = self.cached_size.load(Ordering::Relaxed);
        if offset >= file_size {
            return Ok(());
        }

        let len = core::cmp::min(file_size - offset, arch::PGSIZE);
        let mut page_cache = self.page_cache.lock();
        let written = self.writeback_raw_at(&frame.slice()[..len], offset)?;
        if written != len {
            return Err(Errno::EIO);
        }
        page_cache.mark_clean(file_page_index);
        Ok(())
    }

    fn release_mmap_shared_page(&self, _file_page_index: usize) {
        self.page_cache.lock().shrink_to_capacity();
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let mut page_cache = self.page_cache.lock();
        self.superblock.lock().get_dent(self.ino, index, &mut page_cache)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        let mut page_cache = self.page_cache.lock();
        self.superblock.lock().lookup(self.ino, name, &mut page_cache)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let mut page_cache = self.page_cache.lock();
        self.superblock.lock().readlink(self.ino, buf, &mut page_cache)
    }

    fn size(&self) -> SysResult<u64> {
        if self.cacheable_file {
            return Ok(self.cached_size.load(Ordering::Relaxed) as u64);
        }
        self.superblock.lock().inode_size(self.ino)
    }

    fn mode(&self) -> SysResult<Mode> {
        self.superblock.lock().inode_mode(self.ino)
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        self.superblock.lock().chmod_inode(self.ino, mode)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        self.superblock.lock().inode_owner(self.ino)
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        self.superblock.lock().chown_inode(self.ino, uid, gid)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.superblock.lock().inode_type(self.ino)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut stat = self.superblock.lock().inode_stat(self.ino)?;
        if self.cacheable_file {
            stat.st_size = self.cached_size.load(Ordering::Relaxed) as i64;
        }
        Ok(stat)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        self.flush_dirty_pages()?;
        self.superblock.lock().truncate_inode(self.ino, new_size)?;
        let new_size = usize::try_from(new_size).map_err(|_| Errno::EFBIG)?;
        self.cached_size.store(new_size, Ordering::Relaxed);
        self.page_cache.lock().discard_after_truncate(new_size);
        Ok(())
    }

    fn fallocate(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()> {
        if !flags.is_empty() {
            return Err(Errno::EOPNOTSUPP);
        }
        let new_size = offset.checked_add(len).ok_or(Errno::EFBIG)?;
        if new_size <= self.size()? {
            return Ok(());
        }
        self.truncate(new_size)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        self.superblock
            .lock()
            .update_inode_times(self.ino, Some(time), None, None)
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        self.superblock
            .lock()
            .update_inode_times(self.ino, None, Some(time), None)
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        self.superblock
            .lock()
            .update_inode_times(self.ino, None, None, Some(time))
    }

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()> {
        let time_sec = time.as_secs() as usize;
        if self.last_write_time_sec.load(Ordering::Relaxed) == time_sec {
            return Ok(());
        }
        self.superblock
            .lock()
            .update_inode_times(self.ino, None, Some(time), Some(time))?;
        self.last_write_time_sec.store(time_sec, Ordering::Relaxed);
        Ok(())
    }

    fn sync(&self) -> SysResult<()> {
        self.flush_dirty_pages()?;
        self.superblock.lock().flush_inode(self.ino)
    }

    fn wrap_file(
        self: Arc<Self>,
        inode: Arc<VfsInode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }
}

impl Drop for Inode {
    fn drop(&mut self) {
        if self.flush_dirty_pages().is_ok() {
            let _ = self.superblock.lock().flush_inode(self.ino);
        }
    }
}
