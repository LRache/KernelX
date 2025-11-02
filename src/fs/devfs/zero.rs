use crate::kernel::uapi::FileStat;
use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode::{Mode, InodeOps};
use crate::fs::file::DirResult;

use super::def::ZERO_INO;
pub struct ZeroInode {
    sno: u32,
}

impl ZeroInode {
    pub fn new(sno: u32) -> Self {
        Self { sno }
    }
}

impl InodeOps for ZeroInode {
    fn get_ino(&self) -> u32 {
        ZERO_INO
    }

    fn get_sno(&self) -> u32 {
        self.sno
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

    fn get_dent(&self, _index: usize) -> SysResult<Option<DirResult>> {
        Err(Errno::ENOTDIR)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = ZERO_INO as u64;
        kstat.st_size = 0;
        kstat.st_mode = Mode::S_IFCHR.bits() as u32 | 0o666;
        Ok(kstat)
    }
}
