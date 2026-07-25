use alloc::sync::Arc;

use crate::arch;
use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, VfsSuperBlockOps};
use crate::fs::{Inode, Mode, VfsInode};
use crate::kernel::errno::{Errno, SysResult};
#[cfg(feature = "fanotify")]
use crate::kernel::event::Fanotify;
use crate::kernel::uapi::Statfs;
#[cfg(feature = "fanotify")]
use crate::klib::LazyInitedCell;

use super::inode;

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(
        &self,
        _fsno: u32,
        _driver: Option<Arc<dyn BlockDriverOps>>,
        _options: MountOptions,
    ) -> SysResult<Arc<dyn VfsSuperBlockOps>> {
        Ok(Arc::new(SuperBlock::new()))
    }
}

pub struct SuperBlock {
    #[cfg(feature = "fanotify")]
    fanotify: LazyInitedCell<Arc<Fanotify>>,
}

impl SuperBlock {
    fn new() -> Self {
        Self {
            #[cfg(feature = "fanotify")]
            fanotify: LazyInitedCell::new("ProcfsSuperBlock::fanotify"),
        }
    }
}

impl VfsSuperBlockOps for SuperBlock {
    fn get_root_ino(&self) -> u32 {
        inode::RootInode::INO
    }

    fn get_inode(&self, ino: u32) -> SysResult<Arc<Inode>> {
        match ino {
            inode::RootInode::INO => Ok(VfsInode::new(inode::RootInode)),
            inode::TaskDirSelfInode::INO => Ok(VfsInode::new(inode::TaskDirSelfInode)),
            inode::MountsInode::INO => Ok(VfsInode::new(inode::MountsInode)),
            inode::MemInfoInode::INO => Ok(VfsInode::new(inode::MemInfoInode)),
            inode::UptimeInode::INO => Ok(VfsInode::new(inode::UptimeInode)),
            inode::SysDirInode::INO => Ok(VfsInode::new(inode::SysDirInode)),
            inode::SysKernelDirInode::INO => Ok(VfsInode::new(inode::SysKernelDirInode)),
            inode::PidMaxInode::INO => Ok(VfsInode::new(inode::PidMaxInode)),
            inode::TaintedInode::INO => Ok(VfsInode::new(inode::TaintedInode)),
            inode::SysKernelRandomDirInode::INO => Ok(VfsInode::new(inode::SysKernelRandomDirInode)),
            inode::EntropyAvailInode::INO => Ok(VfsInode::new(inode::EntropyAvailInode)),
            inode::SysFsDirInode::INO => Ok(VfsInode::new(inode::SysFsDirInode)),
            inode::PipeMaxSizeInode::INO => Ok(VfsInode::new(inode::PipeMaxSizeInode)),
            inode::PipeUserPagesSoftInode::INO => Ok(VfsInode::new(inode::PipeUserPagesSoftInode)),
            inode::SysVmDirInode::INO => Ok(VfsInode::new(inode::SysVmDirInode)),
            inode::VfsCachePressureInode::INO => Ok(VfsInode::new(inode::VfsCachePressureInode)),
            inode::DropCachesInode::INO => Ok(VfsInode::new(inode::DropCachesInode)),
            i if i >= inode::TaskDirInode::BASE_INO && i < inode::TaskMapsInode::INO_BASE => {
                Ok(VfsInode::new(inode::TaskDirInode::from_ino(i).ok_or(Errno::ENOENT)?))
            }
            i if i >= inode::TaskMapsInode::INO_BASE && i < inode::TaskExeInode::INO_BASE => {
                Ok(VfsInode::new(inode::TaskMapsInode::from_ino(i).ok_or(Errno::ENOENT)?))
            }
            i if i >= inode::TaskExeInode::INO_BASE && i < inode::TaskStatInode::INO_BASE => {
                Ok(VfsInode::new(inode::TaskExeInode::from_ino(i).ok_or(Errno::ENOENT)?))
            }
            i if i >= inode::TaskStatInode::INO_BASE && i < inode::TaskStatusInode::INO_BASE => {
                Ok(VfsInode::new(inode::TaskStatInode::from_ino(i).ok_or(Errno::ENOENT)?))
            }
            i if i >= inode::TaskStatusInode::INO_BASE && i < inode::TaskFdDirInode::INO_BASE => {
                Ok(VfsInode::new(inode::TaskStatusInode::from_ino(i).ok_or(Errno::ENOENT)?))
            }
            i if i >= inode::TaskFdDirInode::INO_BASE && i < inode::TaskFdEntryInode::INO_BASE => {
                Ok(VfsInode::new(inode::TaskFdDirInode::from_ino(i).ok_or(Errno::ENOENT)?))
            }
            i if i >= inode::TaskFdEntryInode::INO_BASE && i < inode::TaskFdEntryInode::INO_END => Ok(VfsInode::new(
                inode::TaskFdEntryInode::from_ino(i).ok_or(Errno::ENOENT)?,
            )),
            i if i >= inode::TaskTaskDirInode::INO_BASE && i < inode::TaskTaskDirInode::INO_END => Ok(VfsInode::new(
                inode::TaskTaskDirInode::from_ino(i).ok_or(Errno::ENOENT)?,
            )),
            i if i >= inode::TaskThreadDirInode::INO_BASE && i < inode::TaskThreadDirInode::INO_END => Ok(
                VfsInode::new(inode::TaskThreadDirInode::from_ino(i).ok_or(Errno::ENOENT)?),
            ),
            i if i >= inode::TaskFdInfoDirInode::INO_BASE && i < inode::TaskFdInfoDirInode::INO_END => Ok(
                VfsInode::new(inode::TaskFdInfoDirInode::from_ino(i).ok_or(Errno::ENOENT)?),
            ),
            i if i >= inode::TaskFdInfoEntryInode::INO_BASE && i < inode::TaskFdInfoEntryInode::INO_END => Ok(
                VfsInode::new(inode::TaskFdInfoEntryInode::from_ino(i).ok_or(Errno::ENOENT)?),
            ),
            _ => Err(Errno::ENOENT),
        }
    }

    #[cfg(feature = "fanotify")]
    fn fanotify(&self) -> Option<Arc<Fanotify>> {
        self.fanotify.get()
    }

    #[cfg(feature = "fanotify")]
    fn ensure_fanotify(&self) -> Option<Arc<Fanotify>> {
        Some(self.fanotify.get_or_init(|| Arc::new(Fanotify::new())))
    }

    fn create_temp(&self, _mode: Mode) -> SysResult<Arc<Inode>> {
        Err(Errno::EROFS)
    }

    fn statfs(&self) -> SysResult<Statfs> {
        let mut statfs = Statfs::default();
        statfs.f_type = 0x9fa0; // PROCFS_MAGIC
        statfs.f_bsize = arch::PGSIZE as u64;
        statfs.f_blocks = 0;
        statfs.f_bfree = 0;
        statfs.f_bavail = 0;
        statfs.f_flag = self.statfs_flags().bits();
        Ok(statfs)
    }

    fn sync(&self) -> SysResult<()> {
        Ok(())
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn type_name(&self) -> &'static str {
        "procfs"
    }
}
