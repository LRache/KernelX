use alloc::sync::Arc;

use crate::fs::file::{DirResult, FileFlags, FileOps};
use crate::fs::{Dentry, Inode, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;
use crate::kvm::VTaskSet;

pub struct KvmInode {
    ino: u32,
}

impl KvmInode {
    pub fn new(ino: u32) -> Self {
        Self { ino }
    }
}

impl InodeOps for KvmInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        Err(Errno::ENOTDIR)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_size = 0;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;
        kstat.st_gid = 0;
        kstat.st_uid = 0;
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(Mode::S_IFCHR.bits() | 0o666))
    }

    fn wrap_file(&self, _inode: Arc<Inode>, _dentry: Option<Arc<Dentry>>, _flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(VTaskSet::new())
    }
}
