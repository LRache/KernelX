use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;

use crate::arch;
use crate::driver::chosen::kclock;
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{Inode as VfsInode, Mode, Owner};
use crate::fs::memtreefs::superblock::{StaticFsInfo, SuperBlockInner};
use crate::fs::{Dentry, FileType};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::ipc::Pipe;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::mm::{AddrSpace, PhysPageFrame};
use crate::kernel::uapi::{FileFallocateFlags, FileStat, Uid};
use crate::klib::SpinLock;

use super::MemInodeOps;
use super::meta::{InodeMeta, Meta};

pub struct RegularInode<T: StaticFsInfo> {
    ino: u32,
    meta: SpinLock<InodeMeta>,
    superblock: Arc<SpinLock<SuperBlockInner<T>>>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: StaticFsInfo> RegularInode<T> {
    pub fn new(ino: u32, meta: InodeMeta, superblock: Arc<SpinLock<SuperBlockInner<T>>>) -> Self {
        Self {
            ino,
            meta: SpinLock::new(meta, "RegularInode::meta"),
            superblock,
            _marker: core::marker::PhantomData,
        }
    }

    pub fn add_child(&self, name: String, child: Arc<dyn MemInodeOps<T>>) -> SysResult<()> {
        if let Meta::Directory(ref mut children) = self.meta.lock().meta {
            T::check_filename(&name)?;

            if children.contains_key(&name) {
                return Err(Errno::EEXIST);
            }

            let ino = child.as_ref().get_ino();
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

    fn create_child(&self, name: &str, mode: Mode, owner: Owner, rdev: u64) -> SysResult<Arc<dyn MemInodeOps<T>>> {
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
            child_meta.rdev = rdev;
            child_meta.links += 1;

            let inode: Arc<dyn MemInodeOps<T>> = Arc::new(Self::new(ino, child_meta, self.superblock.clone()));
            children.insert(name.into(), ino);

            meta.links += 1;

            sb.insert_inode(ino, inode.clone());

            Ok(inode)
        } else {
            Err(Errno::ENOTDIR)
        }
    }
}

impl<T: StaticFsInfo> MemInodeOps<T> for RegularInode<T> {
    fn as_regular(&self) -> Option<&RegularInode<T>> {
        Some(self)
    }

    fn get_ino(&self) -> u32 {
        RegularInode::<T>::get_ino(self)
    }

    fn type_name(&self) -> &'static str {
        RegularInode::<T>::type_name(self)
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn MemInodeOps<T>>> {
        self.create_child(name, mode, owner, 0)
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Arc<dyn MemInodeOps<T>>> {
        match mode & Mode::S_IFMT {
            Mode::S_IFCHR | Mode::S_IFBLK => self.create_child(name, mode, owner, dev),
            Mode::S_IFIFO => self.create_child(name, mode, owner, 0),
            _ => Err(Errno::EOPNOTSUPP),
        }
    }

    fn add_child(&self, name: String, child: Arc<dyn MemInodeOps<T>>) -> SysResult<()> {
        self.add_child(name, child)
    }

    fn link(&self, name: &str, target: &dyn MemInodeOps<T>) -> SysResult<()> {
        let target = target.as_regular().ok_or(Errno::EXDEV)?;
        RegularInode::<T>::link(self, name, target)
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        RegularInode::<T>::unlink(self, name)
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        RegularInode::<T>::symlink(self, target)
    }

    fn readat(&self, buf: &mut [u8], offset: usize, direct: bool) -> SysResult<usize> {
        RegularInode::<T>::readat(self, buf, offset, direct)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        RegularInode::<T>::writeat(self, buf, offset)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        RegularInode::<T>::read_to_user(self, ubuf, offset, direct)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer, offset: usize, direct: bool) -> SysResult<usize> {
        RegularInode::<T>::write_from_user(self, ubuf, offset, direct)
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        RegularInode::<T>::get_dent(self, index)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        RegularInode::<T>::lookup(self, name)
    }

    fn rename(&self, old_name: &str, new_parent: &dyn MemInodeOps<T>, new_name: &str) -> SysResult<()> {
        let new_parent = new_parent.as_regular().ok_or(Errno::EXDEV)?;
        RegularInode::<T>::rename(self, old_name, new_parent, new_name)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        RegularInode::<T>::readlink(self, buf)
    }

    fn follow_magic_link(&self) -> SysResult<Option<Arc<Dentry>>> {
        Ok(None)
    }

    fn size(&self) -> SysResult<u64> {
        RegularInode::<T>::size(self)
    }

    fn mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        RegularInode::<T>::mmap_shared_page(self, file_page_index)
    }

    fn writeback_mmap_shared_page(&self, file_page_index: usize, frame: &PhysPageFrame) -> SysResult<()> {
        RegularInode::<T>::writeback_mmap_shared_page(self, file_page_index, frame)
    }

    fn mode(&self) -> SysResult<Mode> {
        RegularInode::<T>::mode(self)
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        RegularInode::<T>::chmod(self, mode)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        RegularInode::<T>::owner(self)
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        RegularInode::<T>::chown(self, uid, gid)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        RegularInode::<T>::inode_type(self)
    }

    fn sync(&self) -> SysResult<()> {
        Ok(())
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let _ = request;
        let _ = arg;
        let _ = addrspace;
        Err(Errno::ENOTTY)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        RegularInode::<T>::fstat(self)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        RegularInode::<T>::truncate(self, new_size)
    }

    fn fallocate(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()> {
        RegularInode::<T>::fallocate(self, flags, offset, len)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        RegularInode::<T>::update_atime(self, time)
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        RegularInode::<T>::update_mtime(self, time)
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        RegularInode::<T>::update_ctime(self, time)
    }

    fn update_mtime_ctime(&self, time: &Duration) -> SysResult<()> {
        RegularInode::<T>::update_mtime(self, time)?;
        RegularInode::<T>::update_ctime(self, time)
    }

    fn open_file(
        &self,
        inode: Arc<VfsInode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> SysResult<Arc<dyn FileOps>> {
        RegularInode::<T>::open_file(self, inode, dentry, flags)
    }

    fn wrap_file(&self, inode: Arc<VfsInode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        RegularInode::<T>::wrap_file(self, inode, dentry, flags)
    }
}

impl<T: StaticFsInfo> RegularInode<T> {
    fn get_ino(&self) -> u32 {
        self.ino
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

    fn link(&self, name: &str, target_inode: &Self) -> SysResult<()> {
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

        let mut total_read = 0;
        let mut current_offset = offset;
        for kbuf in ubuf.iter_mut() {
            let mut kbuf = kbuf?;
            let n = self.readat(&mut kbuf, current_offset, direct)?;
            total_read += n;
            current_offset += n;
            if n < kbuf.len() {
                return Ok(total_read);
            }
        }
        Ok(total_read)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        let mut inode_meta = self.meta.lock();
        if let Meta::File(ref mut meta) = inode_meta.meta {
            if buf.is_empty() {
                return Ok(0);
            }

            let mut written_bytes = 0;
            let mut current_offset = offset;

            while written_bytes < buf.len() {
                let page_index = current_offset / arch::PGSIZE;
                let page_offset = current_offset % arch::PGSIZE;

                let to_write = core::cmp::min(buf.len() - written_bytes, arch::PGSIZE - page_offset);
                let page = meta
                    .pages
                    .entry(page_index)
                    .or_insert_with(|| Arc::new(PhysPageFrame::alloc_zeroed()));

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
        {
            let inode_meta = self.meta.lock();
            let Meta::File(_) = inode_meta.meta else {
                return Err(Errno::EINVAL);
            };
            if ubuf.length() == 0 {
                return Ok(0);
            }
        }

        let mut written_bytes = 0;
        let mut current_offset = offset;
        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            let n = self.writeat(&kbuf, current_offset)?;
            written_bytes += n;
            current_offset += n;
            if n < kbuf.len() {
                return Ok(written_bytes);
            }
        }
        Ok(written_bytes)
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if let Meta::Directory(children) = &mut meta.meta {
            T::check_filename(name)?;

            let ino = *children.get(name).ok_or(Errno::ENOENT)?;
            let child = self.superblock.lock().get_inode(ino)?;
            let now = kclock::now().unwrap_or_default();

            let remove_inode = if let Some(child_inode) = child.as_ref().as_regular() {
                // SAFETY: parent and child are distinct inode instances here; the
                // current lockdep model keys only by class name and reports this
                // parent->child meta nesting as same-lock recursion.
                let mut child_meta = unsafe { child_inode.meta.lock_unchecked() };
                if let Meta::Directory(grandchildren) = &child_meta.meta
                    && grandchildren.len() > 2
                {
                    return Err(Errno::ENOTEMPTY);
                }

                child_meta.links = child_meta.links.saturating_sub(1);
                child_meta.ctime = now;
                child_meta.links == 0
            } else {
                if child.as_ref().inode_type()? == FileType::Directory {
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

    fn rename(&self, old_name: &str, new_parent: &Self, new_name: &str) -> SysResult<()> {
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
            let target_inode = target.as_ref().as_regular().ok_or(Errno::EIO)?;
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
        let source_inode = source.as_ref().as_regular().ok_or(Errno::EIO)?;
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
                let ancestor_inode = ancestor.as_ref().as_regular().ok_or(Errno::EIO)?;
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

        drop(old_parent_meta);

        let (mut old_parent_meta, mut new_parent_meta) = if self.ino < new_parent.ino {
            let old_parent_meta = self.meta.lock();
            // SAFETY: self and new_parent are distinct inode instances in this
            // branch, and both parent meta locks are acquired in ascending ino
            // order, so bypassing the same-name lockdep check cannot hide an
            // AB-BA cycle between these two locks.
            let new_parent_meta = unsafe { new_parent.meta.lock_unchecked() };
            (old_parent_meta, new_parent_meta)
        } else {
            let new_parent_meta = new_parent.meta.lock();
            // SAFETY: self and new_parent are distinct inode instances in this
            // branch, and both parent meta locks are acquired in ascending ino
            // order, so bypassing the same-name lockdep check cannot hide an
            // AB-BA cycle between these two locks.
            let old_parent_meta = unsafe { self.meta.lock_unchecked() };
            (old_parent_meta, new_parent_meta)
        };
        let Meta::Directory(old_children) = &mut old_parent_meta.meta else {
            return Err(Errno::ENOTDIR);
        };
        let Meta::Directory(new_children) = &mut new_parent_meta.meta else {
            return Err(Errno::ENOTDIR);
        };

        if old_children.get(old_name) != Some(&old_ino) {
            return Err(Errno::ENOENT);
        }

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

    fn mmap_shared_page(&self, file_page_index: usize) -> SysResult<Option<Arc<PhysPageFrame>>> {
        let offset = file_page_index.checked_mul(arch::PGSIZE).ok_or(Errno::EFBIG)?;
        let mut inode_meta = self.meta.lock();
        if let Meta::File(ref mut meta) = inode_meta.meta {
            if offset >= meta.filesize {
                return Ok(None);
            }

            let frame = meta
                .pages
                .entry(file_page_index)
                .or_insert_with(|| Arc::new(PhysPageFrame::alloc_zeroed()))
                .clone();
            Ok(Some(frame))
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn writeback_mmap_shared_page(&self, _file_page_index: usize, _frame: &PhysPageFrame) -> SysResult<()> {
        Ok(())
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(self.meta.lock().mode)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.mode().map(|mode| mode.into())
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
        kstat.st_rdev = meta.rdev;
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
            let new_size = usize::try_from(new_size).map_err(|_| Errno::EFBIG)?;

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

    fn fallocate(&self, flags: FileFallocateFlags, offset: u64, len: u64) -> SysResult<()> {
        if flags.is_empty() {
            let new_size = offset.checked_add(len).ok_or(Errno::EFBIG)?;
            if new_size <= self.size()? {
                return Ok(());
            }

            return self.truncate(new_size);
        }

        if flags != (FileFallocateFlags::FALLOC_FL_KEEP_SIZE | FileFallocateFlags::FALLOC_FL_PUNCH_HOLE) {
            return Err(Errno::EINVAL);
        }

        let mut meta = self.meta.lock();
        if let Meta::File(ref mut file_meta) = meta.meta {
            let offset = usize::try_from(offset).map_err(|_| Errno::EFBIG)?;
            let len = usize::try_from(len).map_err(|_| Errno::EFBIG)?;
            if len == 0 || offset >= file_meta.filesize {
                return Ok(());
            }

            let end = offset.checked_add(len).ok_or(Errno::EFBIG)?.min(file_meta.filesize);
            if offset >= end {
                return Ok(());
            }

            let start_page = offset / arch::PGSIZE;
            let end_page = (end - 1) / arch::PGSIZE;
            let start_offset = offset % arch::PGSIZE;
            let end_offset = end % arch::PGSIZE;

            if start_page == end_page {
                if let Some(page) = file_meta.pages.get(&start_page) {
                    page.slice()[start_offset..end - start_page * arch::PGSIZE].fill(0);
                }
                return Ok(());
            }

            let first_full_page = if start_offset == 0 {
                start_page
            } else {
                if let Some(page) = file_meta.pages.get(&start_page) {
                    page.slice()[start_offset..].fill(0);
                }
                start_page + 1
            };

            let last_full_page = if end_offset == 0 {
                end_page
            } else {
                if let Some(page) = file_meta.pages.get(&end_page) {
                    page.slice()[..end_offset].fill(0);
                }
                end_page.saturating_sub(1)
            };

            for page_index in first_full_page..=last_full_page {
                file_meta.pages.remove(&page_index);
            }

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

                let inode = self.superblock.lock().get_inode(ino)?;
                let file_type = inode.as_ref().inode_type()?;

                let result = DirResult { ino, name, file_type };
                Ok(Some((result, index + 1)))
            } else {
                Ok(None)
            }
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn open_file(
        &self,
        inode: Arc<VfsInode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> SysResult<Arc<dyn FileOps>> {
        if (self.mode()? & Mode::S_IFMT) == Mode::S_IFIFO {
            let inner = inode.fifo_pipe_inner();
            return Pipe::open_fifo(inner, inode, dentry, flags);
        }

        Ok(self.wrap_file(inode, dentry, flags))
    }

    fn wrap_file(&self, inode: Arc<VfsInode>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }

    fn type_name(&self) -> &'static str {
        T::type_name()
    }
}
