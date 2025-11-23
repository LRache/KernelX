use core::time::Duration;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lwext4_rust::{FileAttr, InodeType};

use crate::fs::FileType;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;
use crate::fs::inode::{InodeOps, Mode};
use crate::fs::file::DirResult;
use crate::klib::SpinLock;

// use super::superblock_inner::{SuperBlockInner, SuperBlockInnerExt, map_error};
use super::superblock::{SuperBlockInner, map_error_to_kernel};

pub struct Ext4Inode {
    ino: u32,
    sno: u32,
    // meta: SpinLock<InodeRef<DummyHal>>,
    superblock: Arc<SpinLock<SuperBlockInner>>,
    dents_cache: SpinLock<Option<Vec<DirResult>>>,
}

impl Ext4Inode {
    pub fn new(sno: u32, ino: u32, superblock: Arc<SpinLock<SuperBlockInner>>) -> Self {
        // let inode_ref = superblock.get_inode_ref(ino);
        // Self {
        //     ino,
        //     sno,
        //     meta: Mutex::new(inode_ref.inode),
        //     superblock: superblock.clone(),
        //     dents_cache: Mutex::new(None),
        // }
        Self { 
            ino, 
            sno, 
            superblock, 
            dents_cache: SpinLock::new(None)
        }
    }

    // fn with_inode_ref<F, R>(&self, f: F) -> R
    // where
    //     F: FnOnce(&mut Ext4InodeRef) -> R,
    // {
    //     let mut meta = self.meta.lock();
    //     let mut inode_ref = Ext4InodeRef {
    //         inode: *meta,
    //         inode_num: self.ino,
    //     };
    //     let r = f(&mut inode_ref);
    //     *meta = inode_ref.inode;
    //     r
    // }

    // fn get_inode_ref(&self, meta: &Meta) -> Ext4InodeRef {
    //     Ext4InodeRef {
    //         inode: *meta,
    //         inode_num: self.ino,
    //     }
    // }
}

impl InodeOps for Ext4Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn get_sno(&self) -> u32 {
        self.sno
    }

    fn type_name(&self) -> &'static str {
        "ext4"
    }

    fn create(&self, name: &str, mode: Mode) -> SysResult<()> {
        if !self.mode()?.contains(Mode::S_IFDIR) {
            return Err(Errno::ENOTDIR);
        }
        
        *self.dents_cache.lock() = None; // Invalidate cache
        let mut superblock = self.superblock.lock();

        let ty = if mode.contains(Mode::S_IFDIR) {
            InodeType::Directory
        } else if mode.contains(Mode::S_IFREG) {
            InodeType::RegularFile
        } else if mode.contains(Mode::S_IFLNK) {
            InodeType::Symlink
        } else if mode.contains(Mode::S_IFCHR) {
            InodeType::CharacterDevice
        } else if mode.contains(Mode::S_IFBLK) {
            InodeType::BlockDevice
        } else if mode.contains(Mode::S_IFIFO) {
            InodeType::Fifo
        } else {
            InodeType::Unknown
        };

        superblock.create(self.ino, name, ty, mode.bits() as u32).map_err(map_error_to_kernel)?;

        Ok(())
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
        // unlink currently not migrated to `lwext4_rust` APIs.
        // Return ENOSYS until a full migration of directory APIs is implemented.
        // let _ = name;
        // Err(Errno::ENOSYS)
        self.superblock.lock().unlink(self.ino, name).map_err(map_error_to_kernel)
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        if self.mode()?.contains(Mode::S_IFDIR) {
            return Err(Errno::EISDIR);
        }
        let mut sb = self.superblock.lock();
        sb.read_at(self.ino, buf, offset as u64).map_err(map_error_to_kernel)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        if self.mode()?.contains(Mode::S_IFDIR) {
            return Err(Errno::EISDIR);
        }
        let mut sb = self.superblock.lock();
        sb.write_at(self.ino, buf, offset as u64).map_err(map_error_to_kernel)
        // kinfo!("Ext4Inode::writeat ino={} offset={} len={}", self.ino, offset, buf.len());
        // self.superblock.get_inode_ref(self.ino);
        // self.superblock.write_at(self.ino, offset, buf).map_err(map_error)
    }

    fn get_dent(&self, offset: usize) -> SysResult<Option<DirResult>> {
        // Directory enumeration not yet migrated to `lwext4_rust`.
        let reader = self.superblock.lock().read_dir(self.ino, offset as u64).map_err(map_error_to_kernel)?;
        let result = reader.current().map(|entry| {
            DirResult {
                ino: entry.ino(),
                name: String::from_utf8_lossy(entry.name()).into_owned(),
                file_type: match entry.inode_type() {
                    InodeType::Directory       => FileType::Directory,
                    InodeType::RegularFile     => FileType::Regular,
                    InodeType::Symlink         => FileType::Symlink,
                    InodeType::CharacterDevice => FileType::CharDevice,
                    InodeType::BlockDevice     => FileType::BlockDevice,
                    InodeType::Fifo            => FileType::FIFO,
                    _                          => FileType::Unknown,
                },
                len: entry.len()
            }
        });
        Ok(result)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        let mut result = self.superblock.lock().lookup(self.ino, name).map_err(map_error_to_kernel)?;
        Ok(result.entry().ino())
    }

    fn rename(&self, old_name: &str, new_parent: &Arc<dyn InodeOps>, new_name: &str) -> SysResult<()> {
        // Rename not yet migrated to `lwext4_rust` directory APIs.
        let _ = (old_name, new_parent, new_name);
        Err(Errno::ENOSYS)
    }

    fn size(&self) -> SysResult<u64> {
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            Ok(inode_ref.size())
        }).map_err(map_error_to_kernel)
    }

    fn mode(&self) -> SysResult<Mode> {
        // Mode::from_bits_truncate(self.meta.lock().mode())
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            Ok(Mode::from_bits_truncate(inode_ref.mode()))
        }).map_err(map_error_to_kernel)
    }

    fn chmod(&self, mode: Mode) -> SysResult<()> {
        debug_assert!(mode.bits() <= 0o777);
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            let current_mode = inode_ref.mode();
            let new_mode = (current_mode & !0o777) | (mode.bits() as u32 & 0o777);
            inode_ref.set_mode(new_mode);
            Ok(())
        }).map_err(map_error_to_kernel)?;
        Ok(())
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        // let meta = self.meta.lock();

        // kstat.st_ino = self.ino as u64;
        // kstat.st_size = meta.size() as i64;
        // kstat.st_nlink = meta.links_count() as u32;
        // kstat.st_mode = meta.mode() as u32;
        // // TODO: utime tests
        // // kstat.st_atime_sec = meta.atime() as i64;
        // kstat.st_atime_sec = 0;
        // kstat.st_atime_nsec = 0;
        // // kstat.st_mtime_sec = meta.mtime() as i64;
        // kstat.st_mtime_sec = 0;
        // kstat.st_mtime_nsec = 0;
        // // kstat.st_ctime_sec = meta.ctime() as i64;
        // kstat.st_ctime_sec = 0;
        // kstat.st_ctime_nsec = 0;

        let mut superblock = self.superblock.lock();

        kstat.st_ino = self.ino as u64;

        superblock.with_inode_ref(self.ino, |inode_ref| {
            let mut attr = FileAttr::default();
            inode_ref.get_attr(&mut attr);

            kstat.st_size = attr.size as i64;
            kstat.st_nlink = attr.nlink as u32;
            kstat.st_mode = attr.mode as u32;
            kstat.st_uid = attr.uid as u32;
            kstat.st_gid = attr.gid as u32;
            kstat.st_blksize = attr.block_size as i32;
            kstat.st_blocks = attr.blocks as u64;

            kstat.st_atime_sec = attr.atime.as_secs() as i64;
            kstat.st_atime_nsec = attr.atime.subsec_nanos() as i64;
            kstat.st_mtime_sec = attr.mtime.as_secs() as i64;
            kstat.st_mtime_nsec = attr.mtime.subsec_nanos() as i64;
            kstat.st_ctime_sec = attr.ctime.as_secs() as i64;
            kstat.st_ctime_nsec = attr.ctime.subsec_nanos() as i64;

            Ok(())
        }).map_err(map_error_to_kernel)?;

        Ok(kstat)
    }

    fn truncate(&self, new_size: u64) -> SysResult<()> {
        self.superblock.lock().set_len(self.ino, new_size).map_err(map_error_to_kernel)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            inode_ref.set_atime(time);
            Ok(())
        }).map_err(map_error_to_kernel)
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            inode_ref.set_mtime(time);
            Ok(())
        }).map_err(map_error_to_kernel)
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            inode_ref.set_ctime(time);
            Ok(())
        }).map_err(map_error_to_kernel)
    }

    fn sync(&self) -> SysResult<()> {
        // sync not implemented in new API yet
        self.superblock.lock().flush().map_err(map_error_to_kernel)
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        // No-op drop until inode lifecycle is migrated to lwext4_rust.
        self.superblock.lock().flush().map_err(map_error_to_kernel).unwrap();
    }
}
