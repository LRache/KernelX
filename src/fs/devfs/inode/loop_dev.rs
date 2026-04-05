use alloc::sync::Arc;

use crate::fs::Dentry;
use crate::fs::file::{DirResult, File, FileFlags, FileOps};
use crate::fs::inode::{InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;

pub struct LoopInode {
    ino: u32,
    minor: u32,
}

impl LoopInode {
    pub fn new(ino: u32, minor: u32) -> Self {
        Self { ino, minor }
    }

    fn rdev(&self) -> u64 {
        // Linux loop device: major 7
        ((7u64) << 8) | self.minor as u64
    }
}

impl InodeOps for LoopInode {
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

    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::ENODEV)
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        Err(Errno::ENOTDIR)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_size = 0;
        kstat.st_mode = Mode::S_IFBLK.bits() as u32 | 0o660;
        kstat.st_nlink = 1;
        kstat.st_rdev = self.rdev();
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(Mode::S_IFBLK.bits() as u32 | 0o660))
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}
