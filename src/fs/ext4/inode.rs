use core::time::Duration;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lwext4_rust::{FileAttr, InodeType};

use crate::driver;
use crate::fs::{Dentry, FileType};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{FileStat, Uid};
use crate::fs::inode::{InodeOps, Mode};
use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::klib::SpinLock;

use super::superblock::{SuperBlockInner, map_error_to_kernel};

pub struct Ext4Inode {
    ino: u32,
    superblock: Arc<SpinLock<SuperBlockInner>>,
    dents_cache: SpinLock<Option<Vec<DirResult>>>,
}

impl Ext4Inode {
    pub fn new(ino: u32, superblock: Arc<SpinLock<SuperBlockInner>>) -> Self {
        Self { 
            ino, 
            superblock, 
            dents_cache: SpinLock::new(None)
        }
    }
}

impl InodeOps for Ext4Inode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "ext4"
    }

    fn create(&self, name: &str, mode: Mode) -> SysResult<Arc<dyn InodeOps>> {
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

        let now = driver::chosen::kclock::now()?;
        let ino = superblock.create(self.ino, name, ty, mode.bits() as u32).map_err(map_error_to_kernel)?;
        superblock.with_inode_ref(ino, |inode_ref| {
            inode_ref.set_atime(&now);
            inode_ref.set_mtime(&now);
            inode_ref.set_ctime(&now);
            Ok(())
        }).map_err(map_error_to_kernel)?;

        Ok(Arc::new(Self::new(ino, self.superblock.clone())))
    }

    fn unlink(&self, name: &str) -> SysResult<()> {
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
    }

    fn get_dent(&self, offset: usize) -> SysResult<Option<(DirResult, usize)>> {
        // Directory enumeration not yet migrated to `lwext4_rust`.
        let mut reader = self.superblock.lock().read_dir(self.ino, offset as u64).map_err(map_error_to_kernel)?;
        let result = reader.current().map(|entry| {
            // kinfo!("Ext4Inode::get_dent ino={} offset={} entry_ino={} name={}", self.ino, offset, entry.ino(), String::from_utf8_lossy(entry.name()));
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
                // len: entry.len()
            }
        });

        if let Some(r) = &result {
            reader.step().map_err(map_error_to_kernel)?;
            let next_offset = reader.offset() as usize;
            Ok(Some((r.clone(), next_offset)))
        } else {
            Ok(None)
        }
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
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            Ok(Mode::from_bits_truncate(inode_ref.mode()))
        }).map_err(map_error_to_kernel)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            if inode_ref.inode_type() != InodeType::Symlink {
                return Ok(None);
            }
            Ok(Some(inode_ref.read_at(buf, 0)?))
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

    fn owner(&self) -> SysResult<(Uid, Uid)> {
        self.superblock.lock().with_inode_ref(self.ino, |inode_ref| {
            Ok((inode_ref.uid() as Uid, inode_ref.gid() as Uid))
        }).map_err(map_error_to_kernel)
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

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}

impl Drop for Ext4Inode {
    fn drop(&mut self) {
        // No-op drop until inode lifecycle is migrated to lwext4_rust.
        self.superblock.lock().flush().map_err(map_error_to_kernel).unwrap();
    }
}
