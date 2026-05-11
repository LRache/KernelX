use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;

use crate::arch;
use crate::driver::chosen::kclock;
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{InodeLockState, Mode, Owner};
use crate::fs::{Dentry, FileType, InodeOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::Fanotify;
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::{FileStat, Uid};
use crate::klib::{LazyInitedCell, SpinLock};

use super::superblock::{StaticFsInfo, SuperBlockInner};

struct FileMeta {
    pages: BTreeMap<usize, PhysPageFrame>,
    filesize: usize,
}

impl FileMeta {
    fn new() -> Self {
        Self {
            pages: BTreeMap::new(),
            filesize: 0,
        }
    }
}

enum Meta {
    File(FileMeta),
    Directory(BTreeMap<String, u32>),
    Symlink(String),
}

pub struct InodeMeta {
    meta: Meta,
    mode: Mode,
    pub(super) owner: (Uid, Uid),
    mtime: Duration,
    atime: Duration,
    ctime: Duration,
    links: u32,
}

impl InodeMeta {
    pub fn new(mode: Mode, ino: u32, parent_ino: u32) -> Self {
        let meta = match mode & Mode::S_IFMT {
            Mode::S_IFDIR => {
                let mut children = BTreeMap::new();
                children.insert(".".into(), ino);
                children.insert("..".into(), parent_ino);
                Meta::Directory(children)
            }
            Mode::S_IFLNK => Meta::Symlink(String::new()),
            _ => Meta::File(FileMeta::new()),
        };
        Self {
            meta,
            mode,
            owner: (0, 0),
            mtime: Duration::ZERO,
            atime: Duration::ZERO,
            ctime: Duration::ZERO,
            links: 0,
        }
    }
}

pub struct Inode<T: StaticFsInfo> {
    ino: u32,
    meta: SpinLock<InodeMeta>,
    lock_state: SpinLock<InodeLockState>,
    fanotify: LazyInitedCell<Arc<Fanotify>>,
    superblock: Arc<SpinLock<SuperBlockInner>>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: StaticFsInfo> Inode<T> {
    pub fn new(ino: u32, meta: InodeMeta, superblock: Arc<SpinLock<SuperBlockInner>>) -> Self {
        Self {
            ino,
            meta: SpinLock::new(meta, "Inode::meta"),
            lock_state: SpinLock::new(InodeLockState::new(), "Inode::lock_state"),
            fanotify: LazyInitedCell::new("Inode::fanotify"),
            superblock,
            _marker: core::marker::PhantomData,
        }
    }

    pub fn add_child(&self, name: String, child: Arc<dyn InodeOps>) -> SysResult<()> {
        if let Meta::Directory(ref mut children) = self.meta.lock().meta {
            T::check_filename(&name)?;

            if children.contains_key(&name) {
                return Err(Errno::EEXIST);
            }

            let ino = child.get_ino();
            children.insert(name, ino);
            self.superblock.lock().insert_inode(ino, child);

            Ok(())
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn pages_for_size(size: usize) -> usize {
        size.div_ceil(arch::PGSIZE)
    }
}

impl<T: StaticFsInfo> InodeOps for Inode<T> {
    fn filesystem_refcount_bias(&self) -> usize {
        1
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        let mut meta = self.meta.lock();
        if let Meta::Directory(ref mut children) = meta.meta {
            T::check_filename(name)?;

            if children.contains_key(name) {
                return Err(Errno::EEXIST);
            }

            let mut sb = self.superblock.lock();
            let ino = sb.alloc_inode_number();

            let mut child_meta = InodeMeta::new(mode, ino, self.ino);
            child_meta.owner = (owner.uid, owner.gid);
            child_meta.links += 1;

            let inode = Arc::new(Self::new(ino, child_meta, self.superblock.clone()));
            children.insert(name.into(), ino);

            meta.links += 1;

            sb.insert_inode(ino, inode.clone());

            Ok(inode)
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify.get()
    }

    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        Some(self.fanotify.get_or_init(|| Arc::new(Fanotify::new())))
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        if let Meta::Directory(ref children) = self.meta.lock().meta {
            T::check_filename(name)?;

            if let Some(&ino) = children.get(name) {
                Ok(ino)
            } else {
                Err(Errno::ENOENT)
            }
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn link(&self, name: &str, target: &Arc<dyn InodeOps>) -> SysResult<()> {
        let target_inode = target.downcast_ref::<Self>().ok_or(Errno::EXDEV)?;
        let existing = self.superblock.lock().get_inode(target_inode.get_ino())?;
        if !Arc::ptr_eq(&existing, target) {
            return Err(Errno::EXDEV);
        }

        if target_inode.inode_type()? == FileType::Directory {
            return Err(Errno::EPERM);
        }

        let mut meta = self.meta.lock();
        if let Meta::Directory(ref mut children) = meta.meta {
            T::check_filename(name)?;

            if children.contains_key(name) {
                return Err(Errno::EEXIST);
            }

            children.insert(name.into(), target_inode.get_ino());

            let now = kclock::now().unwrap_or_default();
            meta.mtime = now;
            meta.ctime = now;

            let mut target_meta = target_inode.meta.lock();
            target_meta.links += 1;
            target_meta.ctime = now;

            Ok(())
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        if let Meta::File(ref file_meta) = self.meta.lock().meta {
            if offset >= file_meta.filesize {
                return Ok(0);
            }

            let mut total_read = 0;
            let mut current_offset = offset;
            let to_read = core::cmp::min(buf.len(), file_meta.filesize - offset);

            while total_read < to_read {
                let page_index = current_offset / arch::PGSIZE;
                let page_offset = current_offset % arch::PGSIZE;
                let len = core::cmp::min(to_read - total_read, arch::PGSIZE - page_offset);
                let dst = &mut buf[total_read..total_read + len];

                if let Some(page) = file_meta.pages.get(&page_index) {
                    page.copy_to_slice(page_offset, dst);
                } else {
                    dst.fill(0);
                }

                total_read += len;
                current_offset += len;
            }

            Ok(total_read)
        } else {
            Err(Errno::EISDIR)
        }
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        if let Meta::File(ref file_meta) = self.meta.lock().meta {
            if direct {
                if offset % arch::PGSIZE != 0 {
                    return Err(Errno::EINVAL);
                }
                if ubuf.length() % arch::PGSIZE != 0 {
                    return Err(Errno::EINVAL);
                }
                if ubuf.uaddr() % arch::PGSIZE != 0 {
                    return Err(Errno::EINVAL);
                }
            }

            if offset >= file_meta.filesize {
                return Ok(0);
            }

            let mut total_read = 0;
            let mut current_offset = offset;
            let mut remaining = file_meta.filesize - offset;

            for kbuf in ubuf.iter_mut() {
                let kbuf = kbuf?;
                if remaining == 0 {
                    break;
                }

                let mut copied = 0;
                let target_len = core::cmp::min(kbuf.len(), remaining);
                while copied < target_len {
                    let page_index = current_offset / arch::PGSIZE;
                    let page_offset = current_offset % arch::PGSIZE;
                    let to_read = core::cmp::min(target_len - copied, arch::PGSIZE - page_offset);
                    let dst = &mut kbuf[copied..copied + to_read];

                    if let Some(page) = file_meta.pages.get(&page_index) {
                        page.copy_to_slice(page_offset, dst);
                    } else {
                        dst.fill(0);
                    }

                    copied += to_read;
                    current_offset += to_read;
                }

                remaining -= copied;
                total_read += copied;
                if copied < kbuf.len() {
                    return Ok(total_read);
                }
            }

            Ok(total_read)
        } else {
            Err(Errno::EISDIR)
        }
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        if let Meta::File(ref mut meta) = self.meta.lock().meta {
            let mut written_bytes = 0;
            let mut current_offset = offset;

            while written_bytes < buf.len() {
                let page_index = current_offset / arch::PGSIZE;
                let page_offset = current_offset % arch::PGSIZE;

                let to_write = core::cmp::min(buf.len() - written_bytes, arch::PGSIZE - page_offset);
                let page = meta.pages.entry(page_index).or_insert_with(PhysPageFrame::alloc_zeroed);

                page.copy_from_slice(page_offset, &buf[written_bytes..written_bytes + to_write]);

                written_bytes += to_write;
                current_offset += to_write;
            }

            meta.filesize = core::cmp::max(meta.filesize, offset + written_bytes);

            Ok(written_bytes)
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, _direct: bool) -> SysResult<usize> {
        if let Meta::File(ref mut meta) = self.meta.lock().meta {
            let mut written_bytes = 0;
            let mut current_offset = offset;

            for kbuf in ubuf.iter() {
                let kbuf = kbuf?;
                let mut copied = 0;
                while copied < kbuf.len() {
                    let page_index = current_offset / arch::PGSIZE;
                    let page_offset = current_offset % arch::PGSIZE;

                    let to_write = core::cmp::min(kbuf.len() - copied, arch::PGSIZE - page_offset);
                    let page = meta.pages.entry(page_index).or_insert_with(PhysPageFrame::alloc_zeroed);
                    page.copy_from_slice(page_offset, &kbuf[copied..copied + to_write]);

                    copied += to_write;
                    current_offset += to_write;
                    written_bytes += to_write;
                }
            }

            meta.filesize = core::cmp::max(meta.filesize, offset + written_bytes);
            Ok(written_bytes)
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if let Meta::Directory(children) = &mut meta.meta {
            T::check_filename(name)?;

            let ino = *children.get(name).ok_or(Errno::ENOENT)?;
            let child = self.superblock.lock().get_inode(ino)?;
            let now = kclock::now().unwrap_or_default();

            let remove_inode = if let Some(child_inode) = child.downcast_ref::<Self>() {
                let mut child_meta = child_inode.meta.lock();
                if let Meta::Directory(grandchildren) = &child_meta.meta
                    && grandchildren.len() > 2
                {
                    return Err(Errno::ENOTEMPTY);
                }

                child_meta.links = child_meta.links.saturating_sub(1);
                child_meta.ctime = now;
                child_meta.links == 0
            } else {
                if child.inode_type()? == FileType::Directory {
                    return Err(Errno::EIO);
                }
                true
            };

            children.remove(name);
            meta.mtime = now;
            meta.ctime = now;
            if remove_inode {
                self.superblock.lock().remove_inode(ino);
            }
            Ok(())
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn rename(&self, old_name: &str, new_parent: &Arc<dyn InodeOps>, new_name: &str) -> SysResult<()> {
        let new_parent = new_parent.downcast_ref::<Self>().ok_or(Errno::EXDEV)?;
        if !Arc::ptr_eq(&self.superblock, &new_parent.superblock) {
            return Err(Errno::EXDEV);
        }
        if old_name == "." || old_name == ".." || new_name == "." || new_name == ".." {
            return Err(Errno::EINVAL);
        }
        if self.ino == new_parent.ino && old_name == new_name {
            return Ok(());
        }

        T::check_filename(old_name)?;
        T::check_filename(new_name)?;

        let now = kclock::now().unwrap_or_default();

        let remove_target = |target_ino: u32, source_is_dir: bool| -> SysResult<()> {
            let target = self.superblock.lock().get_inode(target_ino)?;
            let target_inode = target.downcast_ref::<Self>().ok_or(Errno::EIO)?;
            let mut target_meta = target_inode.meta.lock();
            let target_is_dir = matches!(target_meta.meta, Meta::Directory(_));

            if source_is_dir && !target_is_dir {
                return Err(Errno::ENOTDIR);
            }
            if !source_is_dir && target_is_dir {
                return Err(Errno::EISDIR);
            }
            if let Meta::Directory(children) = &target_meta.meta
                && children.len() > 2
            {
                return Err(Errno::ENOTEMPTY);
            }

            target_meta.links = target_meta.links.saturating_sub(1);
            target_meta.ctime = now;
            let remove_inode = target_meta.links == 0;
            drop(target_meta);

            if remove_inode {
                self.superblock.lock().remove_inode(target_ino);
            }
            Ok(())
        };

        let mut old_parent_meta = self.meta.lock();
        let Meta::Directory(old_children) = &mut old_parent_meta.meta else {
            return Err(Errno::ENOTDIR);
        };

        let old_ino = *old_children.get(old_name).ok_or(Errno::ENOENT)?;
        let source = self.superblock.lock().get_inode(old_ino)?;
        let source_inode = source.downcast_ref::<Self>().ok_or(Errno::EIO)?;
        let source_is_dir = source_inode.inode_type()? == FileType::Directory;

        if self.ino == new_parent.ino {
            if let Some(&target_ino) = old_children.get(new_name) {
                if target_ino == old_ino {
                    return Ok(());
                }
                remove_target(target_ino, source_is_dir)?;
                old_children.remove(new_name);
            }

            old_children.remove(old_name);
            old_children.insert(new_name.into(), old_ino);
            old_parent_meta.mtime = now;
            old_parent_meta.ctime = now;

            let mut source_meta = source_inode.meta.lock();
            source_meta.ctime = now;

            return Ok(());
        }

        if old_ino == new_parent.ino {
            return Err(Errno::EINVAL);
        }

        if source_is_dir {
            let mut ancestor_ino = new_parent.ino;
            loop {
                if ancestor_ino == old_ino {
                    return Err(Errno::EINVAL);
                }
                if ancestor_ino == self.ino {
                    break;
                }

                let ancestor = self.superblock.lock().get_inode(ancestor_ino)?;
                let ancestor_inode = ancestor.downcast_ref::<Self>().ok_or(Errno::EIO)?;
                let ancestor_meta = ancestor_inode.meta.lock();
                let Meta::Directory(ancestor_children) = &ancestor_meta.meta else {
                    return Err(Errno::ENOTDIR);
                };
                let parent_ino = *ancestor_children.get("..").ok_or(Errno::ENOENT)?;
                if parent_ino == ancestor_ino {
                    break;
                }
                ancestor_ino = parent_ino;
            }
        }

        let mut new_parent_meta = new_parent.meta.lock();
        let Meta::Directory(new_children) = &mut new_parent_meta.meta else {
            return Err(Errno::ENOTDIR);
        };

        if let Some(&target_ino) = new_children.get(new_name) {
            if target_ino == old_ino {
                return Ok(());
            }
            remove_target(target_ino, source_is_dir)?;
            new_children.remove(new_name);
        }

        old_children.remove(old_name);
        new_children.insert(new_name.into(), old_ino);
        old_parent_meta.mtime = now;
        old_parent_meta.ctime = now;
        new_parent_meta.mtime = now;
        new_parent_meta.ctime = now;

        let mut source_meta = source_inode.meta.lock();
        if let Meta::Directory(source_children) = &mut source_meta.meta {
            source_children.insert("..".into(), new_parent.ino);
        }
        source_meta.ctime = now;

        Ok(())
    }

    fn size(&self) -> SysResult<u64> {
        let size = match self.meta.lock().meta {
            Meta::File(ref meta) => meta.filesize,
            Meta::Directory(_) => arch::PGSIZE,
            Meta::Symlink(ref target) => target.len(),
        };
        Ok(size as u64)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(self.meta.lock().mode)
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        let mut meta = self.meta.lock();
        let file_type = meta.mode & Mode::S_IFMT;
        meta.mode = file_type | (mode & !Mode::S_IFMT);
        Ok(())
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        Ok(self.meta.lock().owner)
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        let mut meta = self.meta.lock();
        let mut update_ctime = false;
        let mut clear_suid_sgid = false;
        if let Some(uid) = uid {
            if meta.owner.0 != uid {
                meta.owner.0 = uid;
                update_ctime = true;
            }
            clear_suid_sgid = true;
        }
        if let Some(gid) = gid {
            if meta.owner.1 != gid {
                meta.owner.1 = gid;
                update_ctime = true;
            }
            clear_suid_sgid = true;
        }

        if clear_suid_sgid && (meta.mode & Mode::S_IFMT) != Mode::S_IFDIR {
            meta.mode.remove(Mode::S_ISUID);
            if meta.mode.contains(Mode::S_IXGRP) {
                meta.mode.remove(Mode::S_ISGID);
            }
        }

        if update_ctime {
            meta.ctime = kclock::now().unwrap_or_default();
        }

        Ok(())
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();

        let meta = self.meta.lock();
        kstat.st_ino = self.ino as u64;
        kstat.st_mode = meta.mode.bits() as u32;
        kstat.st_blksize = arch::PGSIZE as i32;
        kstat.st_nlink = match &meta.meta {
            Meta::Directory(_) => 1,
            _ => meta.links,
        };
        kstat.st_atime_sec = meta.atime.as_secs() as i64;
        kstat.st_atime_nsec = meta.atime.subsec_nanos() as i64;
        kstat.st_mtime_sec = meta.mtime.as_secs() as i64;
        kstat.st_mtime_nsec = meta.mtime.subsec_nanos() as i64;
        kstat.st_ctime_sec = meta.ctime.as_secs() as i64;
        kstat.st_ctime_nsec = meta.ctime.subsec_nanos() as i64;
        kstat.st_uid = meta.owner.0;
        kstat.st_gid = meta.owner.1;

        match &meta.meta {
            Meta::File(meta) => {
                kstat.st_size = meta.filesize as i64;
                kstat.st_blocks = meta.pages.len() as u64;
            }
            Meta::Directory(_) => {
                kstat.st_size = arch::PGSIZE as i64;
                kstat.st_blocks = 1;
            }
            Meta::Symlink(target) => {
                kstat.st_size = target.len() as i64;
                kstat.st_blocks = 0;
            }
        }

        Ok(kstat)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if let Meta::File(ref mut file_meta) = meta.meta {
            let new_size = new_size as usize;
            if new_size < file_meta.filesize {
                let new_pages = Self::pages_for_size(new_size);
                let _ = file_meta.pages.split_off(&new_pages);

                if new_size % arch::PGSIZE != 0
                    && let Some(page) = file_meta.pages.get(&(new_pages - 1))
                {
                    page.slice()[new_size % arch::PGSIZE..].fill(0);
                }
            }

            file_meta.filesize = new_size;
            Ok(())
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if let Meta::Symlink(ref mut link) = meta.meta {
            link.clear();
            link.push_str(target);
            let now = kclock::now().unwrap_or_default();
            meta.mtime = now;
            meta.ctime = now;
            Ok(())
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let mut meta = self.meta.lock();
        if let Meta::Symlink(ref link) = meta.meta {
            let len = core::cmp::min(link.len(), buf.len());
            buf[..len].copy_from_slice(&link.as_bytes()[..len]);
            meta.atime = kclock::now().unwrap_or_default();
            Ok(Some(len))
        } else {
            Ok(None)
        }
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.atime = *time;
        Ok(())
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.mtime = *time;
        Ok(())
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.ctime = *time;
        Ok(())
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        let meta = self.meta.lock();
        if let Meta::Directory(ref children) = meta.meta {
            if meta.links == 0 {
                return Err(Errno::ENOENT);
            }
            if let Some((name, &ino)) = children.iter().nth(index) {
                if ino == self.ino {
                    // skip "."
                    return Ok(Some((
                        DirResult {
                            ino,
                            name: name.clone(),
                            file_type: FileType::Directory,
                        },
                        index + 1,
                    )));
                }

                let name = name.clone();

                drop(meta);

                let file_type = {
                    let sb = self.superblock.lock();
                    let inode = sb.get_inode(ino)?;
                    inode.inode_type()?
                };

                let result = DirResult { ino, name, file_type };
                Ok(Some((result, index + 1)))
            } else {
                Ok(None)
            }
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(self, dentry.unwrap(), flags))
    }

    fn type_name(&self) -> &'static str {
        T::type_name()
    }
}
