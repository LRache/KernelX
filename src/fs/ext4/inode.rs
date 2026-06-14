use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;
use core::{mem, slice};
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::driver::chosen::kclock;
use crate::fs::ext4::ffi::*;
use crate::fs::ext4::superblock::{SuperBlockInner, map_error_to_kernel};
use crate::fs::ext4::util::{get_block_size, revision_tuple};
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{InodeLockState, InodeOps, Mode, Owner};
use crate::fs::{Dentry, FileType};
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
#[cfg(feature = "fanotify")]
use crate::kernel::event::Fanotify;
use crate::kernel::ipc::Pipe;
use crate::kernel::ipc::pipe::PipeInner;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::uapi::{FileStat, Uid};
#[cfg(feature = "fanotify")]
use crate::klib::LazyInitedCell;
use crate::klib::{SleepLock, SpinLock};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum InodeType {
    Unknown,
    Fifo,
    CharacterDevice,
    Directory,
    BlockDevice,
    RegularFile,
    Symlink,
    Socket,
}

fn ext4_result(code: i32) -> SysResult<()> {
    if code == EOK as i32 {
        Ok(())
    } else {
        Err(map_error_to_kernel(code))
    }
}

fn now() -> Duration {
    kclock::now().unwrap_or(Duration::ZERO)
}

fn encode_time(dur: &Duration) -> u32 {
    dur.as_secs() as u32
}

fn inode_mode(inode_ref: &mut ext4_inode_ref) -> u32 {
    unsafe { ext4_inode_get_mode(&mut (*inode_ref.fs).sb, inode_ref.inode) }
}

fn inode_type(inode_ref: &mut ext4_inode_ref) -> InodeType {
    match (inode_mode(inode_ref) >> 12) as u8 {
        1 => InodeType::Fifo,
        2 => InodeType::CharacterDevice,
        4 => InodeType::Directory,
        6 => InodeType::BlockDevice,
        8 => InodeType::RegularFile,
        10 => InodeType::Symlink,
        12 => InodeType::Socket,
        _ => InodeType::Unknown,
    }
}

fn inode_size(inode_ref: &mut ext4_inode_ref) -> u64 {
    unsafe { ext4_inode_get_size(&mut (*inode_ref.fs).sb, inode_ref.inode) }
}

fn inode_blocks(inode_ref: &mut ext4_inode_ref) -> u64 {
    unsafe { ext4_inode_get_blocks_count(&mut (*inode_ref.fs).sb, inode_ref.inode) }
}

fn inode_flags(inode_ref: &ext4_inode_ref) -> u32 {
    unsafe { ext4_inode_get_flags(inode_ref.inode) }
}

fn inode_has_flags(inode_ref: &ext4_inode_ref, flags: u32) -> bool {
    inode_flags(inode_ref) & flags != 0
}

fn inode_set_flags(inode_ref: &mut ext4_inode_ref, flags: u32) {
    unsafe {
        ext4_inode_set_flags(inode_ref.inode, flags);
    }
    inode_ref.dirty = true;
}

fn inode_nlink(inode_ref: &ext4_inode_ref) -> u16 {
    unsafe { u16::from_le((*inode_ref.inode).links_count) }
}

fn inode_uid(inode_ref: &ext4_inode_ref) -> u16 {
    unsafe { u16::from_le((*inode_ref.inode).uid) }
}

fn inode_gid(inode_ref: &ext4_inode_ref) -> u16 {
    unsafe { u16::from_le((*inode_ref.inode).gid) }
}

fn inode_set_mode(inode_ref: &mut ext4_inode_ref, mode: u32) {
    unsafe {
        ext4_inode_set_mode(&mut (*inode_ref.fs).sb, inode_ref.inode, mode);
    }
    inode_ref.dirty = true;
}

fn inode_set_owner(inode_ref: &mut ext4_inode_ref, uid: u16, gid: u16) {
    unsafe {
        (*inode_ref.inode).uid = u16::to_le(uid);
        (*inode_ref.inode).gid = u16::to_le(gid);
    }
    inode_ref.dirty = true;
}

fn inode_set_atime(inode_ref: &mut ext4_inode_ref, dur: &Duration) {
    unsafe {
        ext4_inode_set_access_time(inode_ref.inode, encode_time(dur));
    }
    inode_ref.dirty = true;
}

fn inode_set_mtime(inode_ref: &mut ext4_inode_ref, dur: &Duration) {
    unsafe {
        ext4_inode_set_modif_time(inode_ref.inode, encode_time(dur));
    }
    inode_ref.dirty = true;
}

fn inode_set_ctime(inode_ref: &mut ext4_inode_ref, dur: &Duration) {
    unsafe {
        ext4_inode_set_change_inode_time(inode_ref.inode, encode_time(dur));
    }
    inode_ref.dirty = true;
}

fn revision_name_len(entry: &ext4_dir_en, sb: &ext4_sblock) -> u16 {
    let mut name_len = entry.name_len as u16;
    if revision_tuple(sb) < (0, 5) {
        let high = unsafe { entry.in_.name_length_high };
        name_len |= (high as u16) << 8;
    }
    name_len
}

fn dir_entry_name<'a>(entry: &'a ext4_dir_en, sb: &ext4_sblock) -> &'a [u8] {
    let name_len = revision_name_len(entry, sb);
    unsafe { slice::from_raw_parts(entry.name.as_ptr(), name_len as usize) }
}

fn dir_entry_type(entry: &ext4_dir_en, sb: &ext4_sblock) -> FileType {
    if revision_tuple(sb) < (0, 5) {
        return FileType::Unknown;
    }

    match unsafe { entry.in_.inode_type } as u32 {
        EXT4_DE_DIR => FileType::Directory,
        EXT4_DE_REG_FILE => FileType::Regular,
        EXT4_DE_SYMLINK => FileType::Symlink,
        EXT4_DE_CHRDEV => FileType::CharDevice,
        EXT4_DE_BLKDEV => FileType::BlockDevice,
        EXT4_DE_FIFO => FileType::FIFO,
        _ => FileType::Unknown,
    }
}

fn lookup_child(parent_ref: &mut ext4_inode_ref, name: &str) -> SysResult<u32> {
    let mut result: ext4_dir_search_result = unsafe { mem::zeroed() };
    let rc = unsafe { ext4_dir_find_entry(&mut result, parent_ref, name.as_ptr().cast(), name.len() as u32) };
    if rc != EOK as i32 {
        return Err(map_error_to_kernel(rc));
    }

    let ino = unsafe { u32::from_le((*result.dentry).inode) };
    let rc = unsafe { ext4_dir_destroy_result(parent_ref, &mut result) };
    ext4_result(rc)?;
    Ok(ino)
}

fn has_children(inode_ref: &mut ext4_inode_ref) -> SysResult<bool> {
    if inode_type(inode_ref) != InodeType::Directory {
        return Ok(false);
    }

    let mut iter: ext4_dir_iter = unsafe { mem::zeroed() };
    let rc = unsafe { ext4_dir_iterator_init(&mut iter, inode_ref, 0) };
    ext4_result(rc)?;

    loop {
        if iter.curr.is_null() {
            break;
        }

        let entry = unsafe { &*(iter.curr as *const ext4_dir_en) };
        let name = dir_entry_name(entry, unsafe { &(*inode_ref.fs).sb });
        if name != b"." && name != b".." {
            unsafe {
                ext4_dir_iterator_fini(&mut iter);
            }
            return Ok(true);
        }

        let rc = unsafe { ext4_dir_iterator_next(&mut iter) };
        ext4_result(rc)?;
    }

    unsafe {
        ext4_dir_iterator_fini(&mut iter);
    }
    Ok(false)
}

fn filetype_from_mode(mode: Mode) -> i32 {
    match mode & Mode::S_IFMT {
        Mode::S_IFDIR => EXT4_DE_DIR as i32,
        Mode::S_IFREG => EXT4_DE_REG_FILE as i32,
        Mode::S_IFLNK => EXT4_DE_SYMLINK as i32,
        Mode::S_IFCHR => EXT4_DE_CHRDEV as i32,
        Mode::S_IFBLK => EXT4_DE_BLKDEV as i32,
        Mode::S_IFIFO => EXT4_DE_FIFO as i32,
        _ => EXT4_DE_UNKNOWN as i32,
    }
}

fn free_unlinked_inode(inode_ref: &mut ext4_inode_ref) -> SysResult<()> {
    unsafe {
        ext4_inode_set_del_time(inode_ref.inode, u32::MAX);
    }
    inode_ref.dirty = true;

    unsafe {
        ext4_result(kernelx_ext4_inode_ref_truncate(inode_ref, 0))?;
        ext4_result(ext4_fs_free_inode(inode_ref))?;
    }
    Ok(())
}

fn check_rename_target_type(source_type: InodeType, target_type: InodeType) -> SysResult<()> {
    if source_type == InodeType::Directory && target_type != InodeType::Directory {
        return Err(Errno::ENOTDIR);
    }
    if source_type != InodeType::Directory && target_type == InodeType::Directory {
        return Err(Errno::EISDIR);
    }
    Ok(())
}

fn unlink_from_parent(
    superblock: &mut SuperBlockInner,
    parent_ref: &mut ext4_inode_ref,
    name: &str,
    rename_source_type: Option<InodeType>,
) -> SysResult<()> {
    let child_ino = lookup_child(parent_ref, name)?;
    let mut child_ref = superblock.read_inode_ref(child_ino)?;

    let result = (|| {
        let child_type = inode_type(&mut child_ref);
        if let Some(source_type) = rename_source_type {
            check_rename_target_type(source_type, child_type)?;
        }

        if child_type == InodeType::Directory && has_children(&mut child_ref)? {
            return Err(Errno::ENOTEMPTY);
        }

        unsafe {
            ext4_result(ext4_dir_remove_entry(
                parent_ref,
                name.as_ptr().cast(),
                name.len() as u32,
            ))?;
        }

        if child_type == InodeType::Directory {
            unsafe {
                ext4_fs_inode_links_count_dec(parent_ref);
                ext4_inode_set_links_cnt(child_ref.inode, 0);
            }
            parent_ref.dirty = true;
            child_ref.dirty = true;
            free_unlinked_inode(&mut child_ref)?;
        } else {
            if inode_nlink(&child_ref) > 0 {
                unsafe {
                    ext4_fs_inode_links_count_dec(&mut child_ref);
                }
                child_ref.dirty = true;
            }

            if inode_nlink(&child_ref) == 0 {
                free_unlinked_inode(&mut child_ref)?;
            }
        }

        Ok(())
    })();

    let put_result = superblock.put_inode_ref(&mut child_ref);
    match result {
        Err(err) => {
            let _ = put_result;
            Err(err)
        }
        Ok(()) => put_result,
    }
}

pub struct Ext4Inode {
    ino: u32,
    superblock: Arc<SleepLock<SuperBlockInner>>,
    lock_state: SpinLock<InodeLockState>,
    mmap_pages: SpinLock<BTreeMap<usize, Arc<PhysPageFrame>>>,
    pipe: SpinLock<Option<Arc<PipeInner>>>,
    #[cfg(feature = "fanotify")]
    fanotify: LazyInitedCell<Arc<Fanotify>>,
}

impl Ext4Inode {
    pub fn new(ino: u32, superblock: Arc<SleepLock<SuperBlockInner>>) -> SysResult<Self> {
        {
            let mut superblock = superblock.lock();
            let mut inode_ref = superblock.read_inode_ref(ino)?;
            superblock.put_inode_ref(&mut inode_ref)?;
        }

        Ok(Self {
            ino,
            superblock,
            lock_state: SpinLock::new(InodeLockState::new(), "Ext4Inode::lock_state"),
            mmap_pages: SpinLock::new(BTreeMap::new(), "Ext4Inode::mmap_pages"),
            pipe: SpinLock::new(None, "Ext4Inode::pipe"),
            #[cfg(feature = "fanotify")]
            fanotify: LazyInitedCell::new("Ext4Inode::fanotify"),
        })
    }

    fn with_ref<R>(&self, f: impl FnOnce(&mut SuperBlockInner, &mut ext4_inode_ref) -> SysResult<R>) -> SysResult<R> {
        let mut superblock = self.superblock.lock();
        let mut inode_ref = superblock.read_inode_ref(self.ino)?;
        let result = f(&mut superblock, &mut inode_ref);
        let put_result = superblock.put_inode_ref(&mut inode_ref);

        match result {
            Err(err) => {
                let _ = put_result;
                Err(err)
            }
            Ok(value) => {
                put_result?;
                Ok(value)
            }
        }
    }

    fn copy_cached_mmap_pages_to_slice(&self, buf: &mut [u8], offset: usize, len: usize) {
        if len == 0 {
            return;
        }

        let pages = self.mmap_pages.lock();
        let mut copied = 0;
        while copied < len {
            let current_offset = offset + copied;
            let page_index = current_offset / arch::PGSIZE;
            let page_offset = current_offset % arch::PGSIZE;
            let copy_len = core::cmp::min(len - copied, arch::PGSIZE - page_offset);

            if let Some(page) = pages.get(&page_index) {
                page.copy_to_slice(page_offset, &mut buf[copied..copied + copy_len]);
            }

            copied += copy_len;
        }
    }

    fn copy_slice_to_cached_mmap_pages(&self, buf: &[u8], offset: usize, len: usize) {
        if len == 0 {
            return;
        }

        let pages = self.mmap_pages.lock();
        let mut copied = 0;
        while copied < len {
            let current_offset = offset + copied;
            let page_index = current_offset / arch::PGSIZE;
            let page_offset = current_offset % arch::PGSIZE;
            let copy_len = core::cmp::min(len - copied, arch::PGSIZE - page_offset);

            if let Some(page) = pages.get(&page_index) {
                page.copy_from_slice(page_offset, &buf[copied..copied + copy_len]);
            }

            copied += copy_len;
        }
    }

    fn discard_cached_mmap_pages_after_truncate(&self, new_size: usize) {
        let mut pages = self.mmap_pages.lock();
        let new_page_count = new_size.div_ceil(arch::PGSIZE);
        let _ = pages.split_off(&new_page_count);

        let tail_offset = new_size % arch::PGSIZE;
        if tail_offset != 0
            && let Some(page) = pages.get(&(new_page_count - 1))
        {
            page.slice()[tail_offset..].fill(0);
        }
    }

    fn fifo_pipe_inner(&self) -> Arc<PipeInner> {
        let mut pipe = self.pipe.lock();
        if let Some(pipe) = &*pipe {
            return pipe.clone();
        }

        let new_pipe = Arc::new(PipeInner::new(config::PIPE_CAPACITY));
        *pipe = Some(new_pipe.clone());
        new_pipe
    }
}

impl InodeOps for Ext4Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "ext4"
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify.get()
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        Some(self.fanotify.get_or_init(|| Arc::new(Fanotify::new())))
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        let child_ino = self.with_ref(|superblock, parent_ref| {
            if inode_type(parent_ref) != InodeType::Directory {
                return Err(Errno::ENOTDIR);
            }

            match lookup_child(parent_ref, name) {
                Ok(_) => return Err(Errno::EEXIST),
                Err(Errno::ENOENT) => {}
                Err(err) => return Err(err),
            }

            let filetype = filetype_from_mode(mode);
            let mut child_ref = superblock.alloc_inode(filetype)?;
            let child_ino = child_ref.index;
            let create_result = (|| {
                unsafe {
                    ext4_result(ext4_dir_add_entry(
                        parent_ref,
                        name.as_ptr().cast(),
                        name.len() as u32,
                        &mut child_ref,
                    ))?;
                }

                if filetype == EXT4_DE_DIR as i32 {
                    let child_ptr: *mut ext4_inode_ref = &mut child_ref;
                    unsafe {
                        ext4_result(ext4_dir_add_entry(child_ptr, c".".as_ptr(), 1, child_ptr))?;
                        if let Err(err) = ext4_result(ext4_dir_add_entry(child_ptr, c"..".as_ptr(), 2, parent_ref)) {
                            let _ = ext4_dir_remove_entry(parent_ref, name.as_ptr().cast(), name.len() as u32);
                            let _ = ext4_dir_remove_entry(child_ptr, c".".as_ptr(), 1);
                            return Err(err);
                        }

                        ext4_inode_set_links_cnt(child_ref.inode, 2);
                        ext4_fs_inode_links_count_inc(parent_ref);
                    }
                    child_ref.dirty = true;
                    parent_ref.dirty = true;
                } else {
                    unsafe {
                        ext4_fs_inode_links_count_inc(&mut child_ref);
                    }
                    child_ref.dirty = true;
                }

                let current_mode = inode_mode(&mut child_ref);
                inode_set_mode(&mut child_ref, (current_mode & !0o7777) | (mode.bits() as u32 & 0o7777));
                inode_set_owner(&mut child_ref, owner.uid as u16, owner.gid as u16);

                let time = now();
                inode_set_atime(&mut child_ref, &time);
                inode_set_mtime(&mut child_ref, &time);
                inode_set_ctime(&mut child_ref, &time);
                inode_set_mtime(parent_ref, &time);
                inode_set_ctime(parent_ref, &time);

                Ok(child_ino)
            })();

            let put_result = superblock.put_inode_ref(&mut child_ref);
            match create_result {
                Err(err) => {
                    unsafe {
                        let _ = ext4_fs_free_inode(&mut child_ref);
                    }
                    let _ = put_result;
                    Err(err)
                }
                Ok(child_ino) => {
                    put_result?;
                    Ok(child_ino)
                }
            }
        })?;

        Ok(Arc::new(Self::new(child_ino, self.superblock.clone())?))
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        self.with_ref(|superblock, parent_ref| {
            if inode_type(parent_ref) != InodeType::Directory {
                return Err(Errno::ENOTDIR);
            }

            unlink_from_parent(superblock, parent_ref, name, None)?;
            let time = now();
            inode_set_mtime(parent_ref, &time);
            inode_set_ctime(parent_ref, &time);
            Ok(())
        })
    }

    fn link(&self, name: &str, target: &Arc<dyn InodeOps>) -> SysResult<()> {
        let target = target.downcast_ref::<Ext4Inode>().ok_or(Errno::EXDEV)?;
        self.with_ref(|superblock, parent_ref| {
            let mut child_ref = superblock.read_inode_ref(target.ino)?;
            let result = (|| {
                unsafe {
                    ext4_result(kernelx_ext4_link(
                        parent_ref,
                        name.as_ptr().cast(),
                        name.len(),
                        &mut child_ref,
                    ))?;
                }

                let time = now();
                inode_set_mtime(parent_ref, &time);
                inode_set_ctime(parent_ref, &time);
                inode_set_ctime(&mut child_ref, &time);
                Ok(())
            })();

            let put_result = superblock.put_inode_ref(&mut child_ref);
            match result {
                Err(err) => {
                    let _ = put_result;
                    Err(err)
                }
                Ok(()) => {
                    put_result?;
                    Ok(())
                }
            }
        })
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        self.with_ref(|_superblock, inode_ref| {
            unsafe {
                ext4_result(kernelx_ext4_inode_ref_set_symlink(
                    inode_ref,
                    target.as_ptr().cast(),
                    target.len(),
                ))?;
            }
            let time = now();
            inode_set_mtime(inode_ref, &time);
            inode_set_ctime(inode_ref, &time);
            Ok(())
        })
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        let read_len = self.with_ref(|_superblock, inode_ref| {
            let mut rcnt = 0usize;
            unsafe {
                ext4_result(kernelx_ext4_inode_ref_read_at(
                    inode_ref,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    offset as u64,
                    &mut rcnt,
                ))?;
            }
            Ok(rcnt)
        })?;
        self.copy_cached_mmap_pages_to_slice(buf, offset, read_len);
        Ok(read_len)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        let written_len = self.with_ref(|_superblock, inode_ref| {
            if inode_has_flags(inode_ref, EXT4_INODE_FLAG_IMMUTABLE) {
                return Err(Errno::EPERM);
            }
            if inode_has_flags(inode_ref, EXT4_INODE_FLAG_APPEND) && offset as u64 != inode_size(inode_ref) {
                return Err(Errno::EPERM);
            }

            let mut wcnt = 0usize;
            let rc = unsafe {
                kernelx_ext4_inode_ref_write_at(inode_ref, buf.as_ptr().cast(), buf.len(), offset as u64, &mut wcnt)
            };
            ext4_result(rc)?;
            Ok(wcnt)
        })?;
        self.copy_slice_to_cached_mmap_pages(buf, offset, written_len);
        Ok(written_len)
    }

    fn get_dent(&self, offset: usize) -> SysResult<Option<(DirResult, usize)>> {
        self.with_ref(|_superblock, inode_ref| {
            if inode_type(inode_ref) != InodeType::Directory {
                return Err(Errno::ENOTDIR);
            }

            let mut iter: ext4_dir_iter = unsafe { mem::zeroed() };
            unsafe {
                ext4_result(ext4_dir_iterator_init(&mut iter, inode_ref, offset as u64))?;
            }

            let current = if iter.curr.is_null() {
                None
            } else {
                let entry = unsafe { &*(iter.curr as *const ext4_dir_en) };
                let sb = unsafe { &(*inode_ref.fs).sb };
                let dent = DirResult {
                    ino: u32::from_le(entry.inode),
                    name: String::from_utf8_lossy(dir_entry_name(entry, sb)).into_owned(),
                    file_type: dir_entry_type(entry, sb),
                };

                unsafe {
                    ext4_result(ext4_dir_iterator_next(&mut iter))?;
                }

                Some((dent, iter.curr_off as usize))
            };

            unsafe {
                ext4_dir_iterator_fini(&mut iter);
            }
            Ok(current)
        })
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        self.with_ref(|_superblock, inode_ref| {
            if inode_type(inode_ref) != InodeType::Directory {
                return Err(Errno::ENOTDIR);
            }

            let ino = lookup_child(inode_ref, name)?;
            inode_set_atime(inode_ref, &now());
            Ok(ino)
        })
    }

    fn rename(&self, old_name: &str, new_parent: &Arc<dyn InodeOps>, new_name: &str) -> SysResult<()> {
        let new_parent = new_parent.downcast_ref::<Ext4Inode>().ok_or(Errno::EXDEV)?;
        if !Arc::ptr_eq(&self.superblock, &new_parent.superblock) {
            return Err(Errno::EXDEV);
        }
        if self.ino == new_parent.ino && old_name == new_name {
            return Ok(());
        }

        let mut superblock = self.superblock.lock();
        let mut src_dir_ref = superblock.read_inode_ref(self.ino)?;

        let result = if self.ino == new_parent.ino {
            (|| {
                let src_ino = lookup_child(&mut src_dir_ref, old_name)?;
                let mut src_ref = superblock.read_inode_ref(src_ino)?;
                let rename_result = (|| {
                    let source_type = inode_type(&mut src_ref);
                    match lookup_child(&mut src_dir_ref, new_name) {
                        Ok(target_ino) if target_ino == src_ino => return Ok(()),
                        Ok(_) => unlink_from_parent(&mut superblock, &mut src_dir_ref, new_name, Some(source_type))?,
                        Err(Errno::ENOENT) => {}
                        Err(err) => return Err(err),
                    }

                    unsafe {
                        ext4_result(ext4_dir_remove_entry(
                            &mut src_dir_ref,
                            old_name.as_ptr().cast(),
                            old_name.len() as u32,
                        ))?;
                        ext4_result(ext4_dir_add_entry(
                            &mut src_dir_ref,
                            new_name.as_ptr().cast(),
                            new_name.len() as u32,
                            &mut src_ref,
                        ))?;
                    }

                    let time = now();
                    inode_set_mtime(&mut src_dir_ref, &time);
                    inode_set_ctime(&mut src_dir_ref, &time);
                    inode_set_ctime(&mut src_ref, &time);
                    Ok(())
                })();
                let put_result = superblock.put_inode_ref(&mut src_ref);
                match rename_result {
                    Err(err) => {
                        let _ = put_result;
                        Err(err)
                    }
                    Ok(()) => {
                        put_result?;
                        Ok(())
                    }
                }
            })()
        } else {
            (|| {
                let mut dst_dir_ref = superblock.read_inode_ref(new_parent.ino)?;
                let outer_result = (|| {
                    let src_ino = lookup_child(&mut src_dir_ref, old_name)?;
                    let mut src_ref = superblock.read_inode_ref(src_ino)?;
                    let rename_result = (|| {
                        let source_type = inode_type(&mut src_ref);
                        match lookup_child(&mut dst_dir_ref, new_name) {
                            Ok(target_ino) if target_ino == src_ino => return Ok(()),
                            Ok(_) => {
                                unlink_from_parent(&mut superblock, &mut dst_dir_ref, new_name, Some(source_type))?
                            }
                            Err(Errno::ENOENT) => {}
                            Err(err) => return Err(err),
                        }

                        if source_type == InodeType::Directory {
                            let mut result: ext4_dir_search_result = unsafe { mem::zeroed() };
                            unsafe {
                                ext4_result(ext4_dir_find_entry(&mut result, &mut src_ref, c"..".as_ptr(), 2))?;
                                (*result.dentry).inode = u32::to_le(new_parent.ino);
                                ext4_trans_set_block_dirty(result.block.buf);
                                ext4_result(ext4_dir_destroy_result(&mut src_ref, &mut result))?;
                                ext4_fs_inode_links_count_dec(&mut src_dir_ref);
                                ext4_fs_inode_links_count_inc(&mut dst_dir_ref);
                            }
                            src_dir_ref.dirty = true;
                            dst_dir_ref.dirty = true;
                        }

                        unsafe {
                            ext4_result(ext4_dir_remove_entry(
                                &mut src_dir_ref,
                                old_name.as_ptr().cast(),
                                old_name.len() as u32,
                            ))?;
                            ext4_result(ext4_dir_add_entry(
                                &mut dst_dir_ref,
                                new_name.as_ptr().cast(),
                                new_name.len() as u32,
                                &mut src_ref,
                            ))?;
                        }

                        let time = now();
                        inode_set_mtime(&mut src_dir_ref, &time);
                        inode_set_ctime(&mut src_dir_ref, &time);
                        inode_set_mtime(&mut dst_dir_ref, &time);
                        inode_set_ctime(&mut dst_dir_ref, &time);
                        inode_set_ctime(&mut src_ref, &time);
                        Ok(())
                    })();

                    let put_src = superblock.put_inode_ref(&mut src_ref);
                    match rename_result {
                        Err(err) => {
                            let _ = put_src;
                            Err(err)
                        }
                        Ok(()) => {
                            put_src?;
                            Ok(())
                        }
                    }
                })();

                let put_dst = superblock.put_inode_ref(&mut dst_dir_ref);
                match outer_result {
                    Err(err) => {
                        let _ = put_dst;
                        Err(err)
                    }
                    Ok(()) => {
                        put_dst?;
                        Ok(())
                    }
                }
            })()
        };

        let put_result = superblock.put_inode_ref(&mut src_dir_ref);
        match result {
            Err(err) => {
                let _ = put_result;
                Err(err)
            }
            Ok(()) => {
                put_result?;
                Ok(())
            }
        }
    }

    fn size(&self) -> SysResult<u64> {
        self.with_ref(|_superblock, inode_ref| Ok(inode_size(inode_ref)))
    }

    fn mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        if let Some(frame) = self.mmap_pages.lock().get(&file_page_index).cloned() {
            return Ok(Some(frame));
        }

        let offset = file_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let file_size = usize::try_from(self.size()?).map_err(|_| Errno::EFBIG)?;
        if offset >= file_size {
            return Ok(None);
        }

        let len = core::cmp::min(file_size - offset, arch::PGSIZE);
        let frame = PhysPageFrame::alloc_zeroed();
        let read_len = self.readat(&mut frame.slice()[..len], offset, false)?;
        if read_len < len {
            frame.slice()[read_len..len].fill(0);
        }

        let frame = Arc::new(frame);
        let mut pages = self.mmap_pages.lock();
        if let Some(existing) = pages.get(&file_page_index) {
            Ok(Some(existing.clone()))
        } else {
            pages.insert(file_page_index, frame.clone());
            Ok(Some(frame))
        }
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        let offset = file_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let file_size = usize::try_from(self.size()?).map_err(|_| Errno::EFBIG)?;
        if offset >= file_size {
            return Ok(());
        }

        let len = core::cmp::min(file_size - offset, arch::PGSIZE);
        self.writeat(&frame.slice()[..len], offset)?;
        Ok(())
    }

    fn release_mmap_shared_page(&self, file_page_index: usize) {
        let mut pages = self.mmap_pages.lock();
        if pages
            .get(&file_page_index)
            .is_some_and(|frame| Arc::strong_count(frame) == 1)
        {
            pages.remove(&file_page_index);
        }
    }

    fn mode(&self) -> SysResult<Mode> {
        self.with_ref(|_superblock, inode_ref| Ok(Mode::from_bits_truncate(inode_mode(inode_ref))))
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        self.with_ref(|_superblock, inode_ref| {
            if inode_type(inode_ref) != InodeType::Symlink {
                return Ok(None);
            }

            let mut rcnt = 0usize;
            unsafe {
                ext4_result(kernelx_ext4_inode_ref_readlink(
                    inode_ref,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    &mut rcnt,
                ))?;
            }
            inode_set_atime(inode_ref, &now());
            Ok(Some(rcnt))
        })
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        debug_assert!(mode.bits() <= 0o7777);
        self.with_ref(|_superblock, inode_ref| {
            let current_mode = inode_mode(inode_ref);
            inode_set_mode(inode_ref, (current_mode & !0o7777) | (mode.bits() as u32 & 0o7777));
            inode_set_ctime(inode_ref, &now());
            Ok(())
        })
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        self.with_ref(|_superblock, inode_ref| {
            let uid = uid.unwrap_or(inode_uid(inode_ref) as Uid) as u16;
            let gid = gid.unwrap_or(inode_gid(inode_ref) as Uid) as u16;
            inode_set_owner(inode_ref, uid, gid);
            let current_mode = inode_mode(inode_ref);
            let cleared_mode = if current_mode & Mode::S_IFMT.bits() == Mode::S_IFDIR.bits() {
                current_mode
            } else if current_mode & Mode::S_IXGRP.bits() != 0 {
                current_mode & !(Mode::S_ISUID | Mode::S_ISGID).bits()
            } else {
                current_mode & !Mode::S_ISUID.bits()
            };
            inode_set_mode(inode_ref, cleared_mode);
            inode_set_ctime(inode_ref, &now());
            Ok(())
        })
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.with_ref(|_superblock, inode_ref| {
            let sb = unsafe { &(*inode_ref.fs).sb };
            let mut stat = FileStat::default();
            stat.st_ino = self.ino as u64;
            stat.st_mode = inode_mode(inode_ref);
            stat.st_nlink = inode_nlink(inode_ref) as u32;
            stat.st_uid = inode_uid(inode_ref) as u32;
            stat.st_gid = inode_gid(inode_ref) as u32;
            stat.st_rdev = 0;
            stat.st_size = inode_size(inode_ref) as i64;
            stat.st_blksize = get_block_size(sb) as i32;
            stat.st_blocks = inode_blocks(inode_ref);
            stat.st_atime_sec = unsafe { ext4_inode_get_access_time(inode_ref.inode) } as i64;
            stat.st_atime_nsec = 0;
            stat.st_mtime_sec = unsafe { ext4_inode_get_modif_time(inode_ref.inode) } as i64;
            stat.st_mtime_nsec = 0;
            stat.st_ctime_sec = unsafe { ext4_inode_get_change_inode_time(inode_ref.inode) } as i64;
            stat.st_ctime_nsec = 0;
            Ok(stat)
        })
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        self.with_ref(|_superblock, inode_ref| {
            if inode_type(inode_ref) == InodeType::Directory {
                return Err(Errno::EISDIR);
            }
            if inode_has_flags(inode_ref, EXT4_INODE_FLAG_IMMUTABLE | EXT4_INODE_FLAG_APPEND) {
                return Err(Errno::EPERM);
            }

            let old_size = inode_size(inode_ref);
            if new_size < old_size {
                unsafe {
                    ext4_result(kernelx_ext4_inode_ref_truncate(inode_ref, new_size))?;
                }
            } else if new_size > old_size {
                unsafe {
                    ext4_inode_set_size(inode_ref.inode, new_size);
                }
                inode_ref.dirty = true;
            }

            Ok(())
        })?;

        let new_size = usize::try_from(new_size).map_err(|_| Errno::EFBIG)?;
        self.discard_cached_mmap_pages_after_truncate(new_size);
        Ok(())
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        self.with_ref(|_superblock, inode_ref| Ok((inode_uid(inode_ref) as Uid, inode_gid(inode_ref) as Uid)))
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        #[derive(TryFromPrimitive)]
        #[allow(non_camel_case_types)]
        #[repr(u32)]
        enum Request {
            FS_IOC_GETFLAGS = 0x80086601,
            FS_IOC_SETFLAGS = 0x40086602,
            FS_IOC32_GETFLAGS = 0x80046601,
            FS_IOC32_SETFLAGS = 0x40046602,
        }

        let request = Request::try_from_primitive(request as u32).map_err(|_| Errno::ENOTTY)?;
        match request {
            Request::FS_IOC_GETFLAGS | Request::FS_IOC32_GETFLAGS => {
                let flags = self.with_ref(|_superblock, inode_ref| Ok(inode_flags(inode_ref)))?;
                addrspace.copy_to_user(arg, flags)?;
                Ok(0)
            }
            Request::FS_IOC_SETFLAGS | Request::FS_IOC32_SETFLAGS => {
                let flags: u32 = addrspace.copy_from_user(arg)?;
                self.with_ref(|_superblock, inode_ref| {
                    inode_set_flags(inode_ref, flags);
                    Ok(())
                })?;
                Ok(0)
            }
        }
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        self.with_ref(|_superblock, inode_ref| {
            inode_set_atime(inode_ref, time);
            Ok(())
        })
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        self.with_ref(|_superblock, inode_ref| {
            inode_set_mtime(inode_ref, time);
            Ok(())
        })
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        self.with_ref(|_superblock, inode_ref| {
            inode_set_ctime(inode_ref, time);
            Ok(())
        })
    }

    fn sync(&self) -> SysResult<()> {
        self.superblock.lock().flush()
    }

    fn open_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> SysResult<Arc<dyn FileOps>> {
        if (self.mode()? & Mode::S_IFMT) == Mode::S_IFIFO {
            let inner = self.fifo_pipe_inner();
            let inode: Arc<dyn InodeOps> = self;
            return Pipe::open_fifo(inner, inode, dentry, flags);
        }

        Ok(self.wrap_file(dentry, flags))
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }
}
