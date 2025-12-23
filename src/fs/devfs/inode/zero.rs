use alloc::sync::Arc;

use crate::fs::Dentry;
use crate::kernel::uapi::FileStat;
use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode::{Mode, InodeOps};
use crate::fs::file::{DirResult, File, FileFlags, FileOps};

pub struct ZeroInode {
    ino: u32,
}

impl ZeroInode {
    pub fn new(ino: u32) -> Self {
        Self { ino }
    }
}

impl InodeOps for ZeroInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn readat(&self, buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        buf.fill(0);
        Ok(buf.len())
    }

    fn writeat(&self, buf: &[u8], _offset: usize) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        Err(Errno::ENOTDIR)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_size = 0;
        kstat.st_mode = Mode::S_IFCHR.bits() as u32 | 0o666;
        kstat.st_nlink = 1;
        kstat.st_gid = 0;
        kstat.st_uid = 0;
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(Mode::S_IFCHR.bits() as u32 | 0o666))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}
