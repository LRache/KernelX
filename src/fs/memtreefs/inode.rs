use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;

use crate::arch;
use crate::driver::chosen::kclock;
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{Inode as VfsInode, InodeLockState, InodeSealOps, Mode, Owner};
use crate::fs::{Dentry, FileType, InodeOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::ipc::Pipe;
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::{FileFallocateFlags, FileSealFlags, FileStat, Uid};
use crate::klib::SpinLock;

use super::superblock::{StaticFsInfo, SuperBlockInner};

struct FileMeta {
    pages: BTreeMap<usize, Arc<PhysPageFrame>>,
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
    rdev: u64,
    links: u32,
    seals: Option<FileSealFlags>,
    shared_mmap_count: usize,
    writable_shared_mmap_count: usize,
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
            rdev: 0,
            links: 0,
            seals: None,
            shared_mmap_count: 0,
            writable_shared_mmap_count: 0,
        }
    }
}

pub struct Inode<T: StaticFsInfo> {
    ino: u32,
    meta: SpinLock<InodeMeta>,
    lock_state: SpinLock<InodeLockState>,
    superblock: Arc<SpinLock<SuperBlockInner>>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: StaticFsInfo> Inode<T> {
    pub fn new(ino: u32, meta: InodeMeta, superblock: Arc<SpinLock<SuperBlockInner>>) -> Self {
        Self {
            ino,
            meta: SpinLock::new(meta, "Inode::meta"),
            lock_state: SpinLock::new(InodeLockState::new(), "Inode::lock_state"),
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

    fn check_write_seals(
        file_meta: &FileMeta,
        seals: Option<FileSealFlags>,
        offset: usize,
        len: usize,
    ) -> SysResult<()> {
        if len == 0 {
            return Ok(());
        }

        let Some(seals) = seals else {
            return Ok(());
        };

        if seals.intersects(FileSealFlags::F_SEAL_WRITE | FileSealFlags::F_SEAL_FUTURE_WRITE) {
            return Err(Errno::EPERM);
        }

        let end = offset.checked_add(len).ok_or(Errno::EFBIG)?;
        if end > file_meta.filesize && seals.contains(FileSealFlags::F_SEAL_GROW) {
            return Err(Errno::EPERM);
        }

        Ok(())
    }

    fn create_child(&self, name: &str, mode: Mode, owner: Owner, rdev: u64) -> SysResult<Arc<dyn InodeOps>> {
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

            let inode = Arc::new(Self::new(ino, child_meta, self.superblock.clone()));
            children.insert(name.into(), ino);

            meta.links += 1;

            sb.insert_inode(ino, inode.clone());

            Ok(inode)
        } else {
            Err(Errno::ENOTDIR)
        }
    }
}

impl<T: StaticFsInfo> InodeSealOps for Inode<T> {
    fn init_seals(&self, seals: FileSealFlags) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if !matches!(meta.meta, Meta::File(_)) {
            return Err(Errno::EINVAL);
        }
        if meta.seals.is_some() {
            return Err(Errno::EINVAL);
        }
        meta.seals = Some(seals);
        Ok(())
    }

    fn seals(&self) -> SysResult<FileSealFlags> {
        self.meta.lock().seals.ok_or(Errno::EINVAL)
    }

    fn add_seals(&self, seals: FileSealFlags) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if !matches!(meta.meta, Meta::File(_)) {
            return Err(Errno::EINVAL);
        }

        let current = meta.seals.ok_or(Errno::EINVAL)?;
        if current.contains(FileSealFlags::F_SEAL_SEAL) {
            return Err(Errno::EPERM);
        }
        if seals.contains(FileSealFlags::F_SEAL_WRITE) && meta.writable_shared_mmap_count > 0 {
            return Err(Errno::EBUSY);
        }

        meta.seals = Some(current | seals);
        Ok(())
    }

    fn begin_shared_mmap(&self, writable: bool) {
        let mut meta = self.meta.lock();
        if !matches!(meta.meta, Meta::File(_)) {
            return;
        }

        meta.shared_mmap_count = meta
            .shared_mmap_count
            .checked_add(1)
            .expect("memtreefs shared_mmap_count overflow");
        if writable {
            meta.writable_shared_mmap_count = meta
                .writable_shared_mmap_count
                .checked_add(1)
                .expect("memtreefs writable_shared_mmap_count overflow");
        }
    }

    fn update_shared_mmap_writable(&self, old_writable: bool, new_writable: bool) {
        if old_writable == new_writable {
            return;
        }

        let mut meta = self.meta.lock();
        if !matches!(meta.meta, Meta::File(_)) {
            return;
        }

        if new_writable {
            meta.writable_shared_mmap_count = meta
                .writable_shared_mmap_count
                .checked_add(1)
                .expect("memtreefs writable_shared_mmap_count overflow");
        } else {
            debug_assert!(
                meta.writable_shared_mmap_count > 0,
                "memtreefs writable mmap count underflow"
            );
            meta.writable_shared_mmap_count = meta.writable_shared_mmap_count.saturating_sub(1);
        }
    }

    fn end_shared_mmap(&self, writable: bool) {
        let mut meta = self.meta.lock();
        if !matches!(meta.meta, Meta::File(_)) {
            return;
        }

        debug_assert!(meta.shared_mmap_count > 0, "memtreefs shared mmap count underflow");
        meta.shared_mmap_count = meta.shared_mmap_count.saturating_sub(1);

        if writable {
            debug_assert!(
                meta.writable_shared_mmap_count > 0,
                "memtreefs writable mmap count underflow"
            );
            meta.writable_shared_mmap_count = meta.writable_shared_mmap_count.saturating_sub(1);
        }
    }
}

impl<T: StaticFsInfo> InodeOps for Inode<T> {
    fn filesystem_refcount_bias(&self) -> usize {
        1
    }

    fn has_dirty_page(&self) -> bool {
        false
    }

    fn as_seal_ops(&self) -> Option<&dyn InodeSealOps> {
        Some(self)
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        self.create_child(name, mode, owner, 0)
    }

    fn mknod(&self, name: &str, mode: Mode, owner: Owner, dev: u64) -> SysResult<Arc<dyn InodeOps>> {
        match mode & Mode::S_IFMT {
            Mode::S_IFCHR | Mode::S_IFBLK => self.create_child(name, mode, owner, dev),
            Mode::S_IFIFO => self.create_child(name, mode, owner, 0),
            _ => Err(Errno::EOPNOTSUPP),
        }
    }

    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
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
            let kbuf = kbuf?;
            let n = self.readat(kbuf, current_offset, direct)?;
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
        let seals = inode_meta.seals;
        if let Meta::File(ref mut meta) = inode_meta.meta {
            if buf.is_empty() {
                return Ok(0);
            }
            Self::check_write_seals(meta, seals, offset, buf.len())?;

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
            let seals = inode_meta.seals;
            let Meta::File(ref meta) = inode_meta.meta else {
                return Err(Errno::EINVAL);
            };
            if ubuf.length() == 0 {
                return Ok(0);
            }
            Self::check_write_seals(meta, seals, offset, ubuf.length())?;
        }

        let mut written_bytes = 0;
        let mut current_offset = offset;
        for kbuf in ubuf.iter() {
            let kbuf = kbuf?;
            let n = self.writeat(kbuf, current_offset)?;
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

            let remove_inode = if let Some(child_inode) = child.downcast_ref::<Self>() {
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

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if meta
            .seals
            .is_some_and(|seals| seals.contains(FileSealFlags::F_SEAL_EXEC))
        {
            let exec_bits = Mode::S_IXUSR | Mode::S_IXGRP | Mode::S_IXOTH;
            if (meta.mode & exec_bits) != (mode & exec_bits) {
                return Err(Errno::EPERM);
            }
        }
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
        let seals = meta.seals;
        if let Meta::File(ref mut file_meta) = meta.meta {
            let new_size = usize::try_from(new_size).map_err(|_| Errno::EFBIG)?;
            if let Some(seals) = seals {
                if new_size < file_meta.filesize && seals.contains(FileSealFlags::F_SEAL_SHRINK) {
                    return Err(Errno::EPERM);
                }
                if new_size > file_meta.filesize && seals.contains(FileSealFlags::F_SEAL_GROW) {
                    return Err(Errno::EPERM);
                }
            }

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
        let seals = meta.seals;
        if let Meta::File(ref mut file_meta) = meta.meta {
            if let Some(seals) = seals
                && seals.intersects(FileSealFlags::F_SEAL_WRITE | FileSealFlags::F_SEAL_FUTURE_WRITE)
            {
                return Err(Errno::EPERM);
            }

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
                let file_type = inode.inode_type()?;

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
        self: Arc<Self>,
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

    fn wrap_file(
        self: Arc<Self>,
        inode: Arc<VfsInode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }

    fn type_name(&self) -> &'static str {
        T::type_name()
    }
}
