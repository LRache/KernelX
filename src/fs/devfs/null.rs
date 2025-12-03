use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;
use crate::fs::{InodeOps, Mode};
use crate::fs::file::DirResult;

pub const INO: u32 = 1;

pub struct NullInode {
    sno: u32,
}

impl NullInode {
    pub fn new(sno: u32) -> Self {
        Self { sno }
    }
}

impl InodeOps for NullInode {
    fn get_ino(&self) -> u32 {
        INO
    }

    fn get_sno(&self) -> u32 {
        self.sno
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        // /dev/null always returns EOF (0 bytes read)
        Ok(0)
    }

    fn writeat(&self, buf: &[u8], _offset: usize) -> SysResult<usize> {
        // /dev/null discards all data but reports success
        Ok(buf.len())
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        // /dev/null is not a directory
        Err(Errno::ENOTDIR)
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();

        kstat.st_ino = INO as u64;
        kstat.st_size = 0;
        kstat.st_mode = Mode::S_IFCHR.bits() as u32 | 0o666;

        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(Mode::S_IFCHR.bits() as u32 | 0o666))
    }
}
