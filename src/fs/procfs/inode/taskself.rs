use core::cmp::min;
use alloc::string::ToString;
use alloc::sync::Arc;

use crate::fs::file::{File, FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::scheduler::current;
use crate::kernel::uapi::FileStat;
use crate::kinfo;

use super::RootInode;

pub struct TaskDirSelfInode;

impl TaskDirSelfInode {
    pub const INO: u32 = 2;
}

impl InodeOps for TaskDirSelfInode {
    fn get_ino(&self) -> u32 {
        Self::INO
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EISDIR)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EROFS)
    }

    fn lookup(&self, name: &str) -> SysResult<u32> {
        match name {
            "." => Ok(Self::INO),
            ".." => Ok(RootInode::INO),
            _ => Err(Errno::ENOENT),
        }
    }

    fn readlink(&self, buffer: &mut [u8]) -> SysResult<Option<usize>> {
        let link_name = current::tid().to_string();
        let bytes = link_name.as_bytes();
        let len = min(buffer.len(), bytes.len());
        buffer[..len].copy_from_slice(&bytes[..len]);
        Ok(Some(len))
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;
        kstat.st_uid = 0;
        kstat.st_gid = 0;
        kstat.st_size = 0;
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::S_IFLNK
            | Mode::S_IRUSR
            | Mode::S_IXUSR
            | Mode::S_IRGRP
            | Mode::S_IXGRP  
            | Mode::S_IROTH
            | Mode::S_IXOTH)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }

    fn type_name(&self) -> &'static str {
        "procfs_task_self"
    }
}
