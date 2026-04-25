use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;

use crate::arch;
use crate::driver::chosen::kclock;
use crate::fs::file::{DirResult, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::{InodeLockState, Mode, Owner};
use crate::fs::{Dentry, FileType, InodeOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::{FileStat, Uid};
use crate::klib::SpinLock;

use super::superblock::{StaticFsInfo, SuperBlockInner};

#[derive(Default)]
struct Timespec {
    tv_sec: u64,
    tv_nsec: u64,
}

struct FileMeta {
    pages: Vec<PhysPageFrame>,
    filesize: usize,
}

impl FileMeta {
    fn new() -> Self {
        Self {
            pages: Vec::new(),
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

    fn zero_file_range(file_meta: &mut FileMeta, start: usize, end: usize) {
        if start >= end {
            return;
        }

        let mut current = start;
        while current < end {
            let page_index = current / arch::PGSIZE;
            let page_offset = current % arch::PGSIZE;
            let chunk_end = core::cmp::min(end, (page_index + 1) * arch::PGSIZE);

            while page_index >= file_meta.pages.len() {
                file_meta.pages.push(PhysPageFrame::alloc_zeroed());
            }

            file_meta.pages[page_index].slice()[page_offset..page_offset + (chunk_end - current)].fill(0);
            current = chunk_end;
        }
    }
}

impl<T: StaticFsInfo> InodeOps for Inode<T> {
    fn filesystem_refcount_bias(&self) -> usize {
        1
    }

    fn create(&self, name: &str, mode: Mode, owner: Owner) -> SysResult<Arc<dyn InodeOps>> {
        let mut meta = self.meta.lock();
        if let Meta::Directory(ref mut children) = meta.meta {
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

    fn lookup(&self, name: &str) -> SysResult<u32> {
        if let Meta::Directory(ref children) = self.meta.lock().meta {
            if let Some(&ino) = children.get(name) {
                Ok(ino)
            } else {
                Err(Errno::ENOENT)
            }
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
            let mut left = core::cmp::min(buf.len(), file_meta.filesize - offset);

            while left > 0 {
                let page_index = current_offset / arch::PGSIZE;
                let page_offset = current_offset % arch::PGSIZE;

                if page_index >= file_meta.pages.len() {
                    break;
                }

                let page = &file_meta.pages[page_index];
                let to_read = core::cmp::min(left, arch::PGSIZE - page_offset);

                page.copy_to_slice(page_offset, &mut buf[total_read..total_read + to_read]);

                left -= to_read;
                total_read += to_read;
                current_offset += to_read;
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
                    if page_index >= file_meta.pages.len() {
                        return Ok(total_read);
                    }

                    let to_read = core::cmp::min(target_len - copied, arch::PGSIZE - page_offset);
                    file_meta.pages[page_index].copy_to_slice(page_offset, &mut kbuf[copied..copied + to_read]);

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
            Err(Errno::EINVAL)
        }
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        if let Meta::File(ref mut meta) = self.meta.lock().meta {
            if offset > meta.filesize {
                Self::zero_file_range(meta, meta.filesize, offset);
            }

            let mut written_bytes = 0;
            let mut current_offset = offset;

            while written_bytes < buf.len() {
                let page_index = current_offset / arch::PGSIZE;
                let page_offset = current_offset % arch::PGSIZE;

                while page_index >= meta.pages.len() {
                    meta.pages.push(PhysPageFrame::alloc_zeroed());
                }

                let page = &meta.pages[page_index];
                let to_write = core::cmp::min(buf.len() - written_bytes, arch::PGSIZE - page_offset);

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
            if offset > meta.filesize {
                Self::zero_file_range(meta, meta.filesize, offset);
            }

            let mut written_bytes = 0;
            let mut current_offset = offset;

            for kbuf in ubuf.iter() {
                let kbuf = kbuf?;
                let mut copied = 0;
                while copied < kbuf.len() {
                    let page_index = current_offset / arch::PGSIZE;
                    let page_offset = current_offset % arch::PGSIZE;

                    while page_index >= meta.pages.len() {
                        meta.pages.push(PhysPageFrame::alloc_zeroed());
                    }

                    let to_write = core::cmp::min(kbuf.len() - copied, arch::PGSIZE - page_offset);
                    meta.pages[page_index].copy_from_slice(page_offset, &kbuf[copied..copied + to_write]);

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
            let ino = *children.get(name).ok_or(Errno::ENOENT)?;
            let child = self.superblock.lock().get_inode(ino)?;
            if child.inode_type()? == FileType::Directory {
                let child_inode = child.downcast_arc::<Self>().map_err(|_| Errno::EIO)?;
                let child_meta = child_inode.meta.lock();
                if let Meta::Directory(grandchildren) = &child_meta.meta
                    && grandchildren.len() > 2
                {
                    return Err(Errno::ENOTEMPTY);
                }
            }

            children.remove(name);
            self.superblock.lock().remove_inode(ino);
            Ok(())
        } else {
            Err(Errno::ENOTDIR)
        }
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

        if clear_suid_sgid && meta.mode.contains(Mode::S_IXGRP) {
            meta.mode.remove(Mode::S_ISUID | Mode::S_ISGID);
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
        kstat.st_nlink = 1;
        kstat.st_atime_sec = meta.atime.as_secs() as i64;
        kstat.st_atime_nsec = meta.atime.subsec_nanos() as i64;
        kstat.st_mtime_sec = meta.mtime.as_secs() as i64;
        kstat.st_mtime_nsec = meta.mtime.subsec_nanos() as i64;
        kstat.st_ctime_sec = meta.ctime.as_secs() as i64;
        kstat.st_ctime_nsec = meta.ctime.subsec_nanos() as i64;
        kstat.st_uid = meta.owner.0;
        kstat.st_gid = meta.owner.1;

        match meta.meta {
            Meta::File(ref meta) => {
                kstat.st_size = meta.filesize as i64;
                kstat.st_blocks = meta.pages.len() as u64;
            }
            Meta::Directory(_) => {
                kstat.st_size = arch::PGSIZE as i64;
                kstat.st_blocks = 1;
            }
            Meta::Symlink(ref target) => {
                kstat.st_size = target.len() as i64;
                kstat.st_blocks = 0;
            }
        }

        Ok(kstat)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if let Meta::File(ref mut file_meta) = meta.meta {
            let old_size = file_meta.filesize;
            let new_size = new_size as usize;
            let new_pages = (new_size + arch::PGSIZE - 1) / arch::PGSIZE;

            if new_size > file_meta.filesize {
                Self::zero_file_range(file_meta, old_size, new_size);
            } else {
                Self::zero_file_range(file_meta, new_size, old_size);

                // Shrink: drop excess pages
                file_meta.pages.truncate(new_pages);
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
