use alloc::sync::Arc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;

use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::kernel::errno::{SysResult, Errno};
use crate::kernel::mm::PhysPageFrame;
use crate::kernel::uapi::{FileStat, Uid};
use crate::fs::{Dentry, FileType, InodeOps};
use crate::fs::inode::Mode;
use crate::arch;
use crate::klib::SpinLock;

use super::superblock::{SuperBlockInner, StaticFsInfo};

struct Timespec {
    tv_sec: u64,
    tv_nsec: u64,
}

impl Default for Timespec {
    fn default() -> Self {
        Timespec { tv_sec: 0, tv_nsec: 0 }
    }
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
}

pub struct InodeMeta {
    meta: Meta,
    mode: Mode,
    owner: (Uid, Uid),
    mtime: Timespec,
    atime: Timespec,
    ctime: Timespec,
    links: u32,
}

impl InodeMeta {
    pub fn new(mode: Mode, ino: u32, parent_ino: u32) -> Self {
        let meta = if mode.contains(Mode::S_IFDIR) {
            let mut children = BTreeMap::new();
            children.insert(".".into(), ino);
            children.insert("..".into(), parent_ino);
            Meta::Directory(children)
        } else {
            Meta::File(FileMeta::new())
        };
        Self {
            meta,
            mode,
            owner: (0, 0),
            mtime: Timespec::default(),
            atime: Timespec::default(),
            ctime: Timespec::default(),
            links: 0,
        }
    }
}

pub struct Inode<T: StaticFsInfo> {
    ino: u32,
    meta: SpinLock<InodeMeta>,
    superblock: Arc<SpinLock<SuperBlockInner>>,
    _marker: core::marker::PhantomData<T>,
}

impl<T: StaticFsInfo> Inode<T> {
    pub fn new(ino: u32, meta: InodeMeta, superblock: Arc<SpinLock<SuperBlockInner>>) -> Self {
        Self {
            ino,
            meta: SpinLock::new(meta),
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
}

impl<T: StaticFsInfo> InodeOps for Inode<T> {
    fn create(&self, name: &str, mode: Mode) -> SysResult<Arc<dyn InodeOps>> {
        let mut meta = self.meta.lock();
        if let Meta::Directory(ref mut children) = meta.meta {
            if children.contains_key(name) {
                return Err(Errno::EEXIST);
            }

            let mut sb = self.superblock.lock();
            let ino = sb.alloc_inode_number();

            let mut child_meta = InodeMeta::new(mode, ino, self.ino);
            child_meta.links += 1;

            let inode = Arc::new(Self::new(
                ino, child_meta,
                self.superblock.clone()
            ));
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

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
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

    fn writeat(&self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        if let Meta::File(ref mut meta) = self.meta.lock().meta {
            let mut written_bytes = 0;
            let mut current_offset = offset;

            while written_bytes < buf.len() {
                let page_index = current_offset / arch::PGSIZE;
                let page_offset = current_offset % arch::PGSIZE;

                while page_index >= meta.pages.len() {
                    meta.pages.push(PhysPageFrame::alloc());
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
            Err(Errno::EISDIR)
        }
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        let mut meta = self.meta.lock();
        if let Meta::Directory(children) = &mut meta.meta {
            let ino = children.remove(name).ok_or(Errno::ENOENT)?;
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
        };
        Ok(size as u64)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(self.meta.lock().mode)
    }

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        Ok(self.meta.lock().owner)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();

        let meta = self.meta.lock();
        kstat.st_ino = self.ino as u64;
        kstat.st_mode = meta.mode.bits() as u32;
        kstat.st_blksize = arch::PGSIZE as i32;
        kstat.st_nlink = 1;
        kstat.st_atime_sec = meta.atime.tv_sec as i64;
        kstat.st_atime_nsec = meta.atime.tv_nsec as i64;
        kstat.st_mtime_sec = meta.mtime.tv_sec as i64;
        kstat.st_mtime_nsec = meta.mtime.tv_nsec as i64;
        kstat.st_ctime_sec = meta.ctime.tv_sec as i64;
        kstat.st_ctime_nsec = meta.ctime.tv_nsec as i64;

        match meta.meta {
            Meta::File(ref meta) => {
                kstat.st_size = meta.filesize as i64;
                kstat.st_blocks = meta.pages.len() as u64;
            }
            Meta::Directory(_) => {
                kstat.st_size = arch::PGSIZE as i64;
                kstat.st_blocks = 1;
            }
        }

        Ok(kstat)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.atime.tv_sec = time.as_secs();
        meta.atime.tv_nsec = time.subsec_nanos() as u64;
        Ok(())
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.mtime.tv_sec = time.as_secs();
        meta.mtime.tv_nsec = time.subsec_nanos() as u64;
        Ok(())
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        let mut meta = self.meta.lock();
        meta.ctime.tv_sec = time.as_secs();
        meta.ctime.tv_nsec = time.subsec_nanos() as u64;
        Ok(())
    }

    fn get_dent(&self, index: usize) -> SysResult<Option<(DirResult, usize)>> {
        if let Meta::Directory(ref children) = self.meta.lock().meta {
            if let Some((name, &ino)) = children.iter().nth(index) {
                if ino == self.ino {
                    // skip "."
                    return Ok(Some((DirResult {
                        ino,
                        name: name.clone(),
                        file_type: FileType::Directory,
                    }, index + 1)));
                }
                
                let file_type = {
                    let sb = self.superblock.lock();
                    let inode = sb.get_inode(ino)?;
                    let mode = inode.mode()?;
                    if mode.contains(Mode::S_IFDIR) {
                        FileType::Directory
                    } else if mode.contains(Mode::S_IFREG) {
                        FileType::Regular
                    } else if mode.contains(Mode::S_IFLNK) {
                        FileType::Symlink
                    } else {
                        FileType::Unknown
                    }
                };

                let result = DirResult {
                    ino,
                    name: name.clone(),
                    file_type,
                };
                Ok(Some((result, index + 1)))
            } else {
                Ok(None)
            }
        } else {
            Err(Errno::ENOTDIR)
        }
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }

    fn type_name(&self) -> &'static str {
        T::type_name()
    }
}
