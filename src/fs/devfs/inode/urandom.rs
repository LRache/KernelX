use alloc::sync::Arc;

use crate::fs::file::{File, FileFlags, FileOps};
use crate::kernel::errno::SysResult;
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::uapi::FileStat;
use crate::klib::random;

pub struct URandomInode {
    ino: u32,
}

impl URandomInode {
    pub fn new(ino: u32) -> Self {
        Self { ino }
    }
}

impl InodeOps for URandomInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn readat(&self, buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        buf.iter_mut().for_each(|b| {
            *b = (random::random() & 0xFF) as u8;
        });
        Ok(buf.len())
    }

    fn writeat(&self, buf: &[u8], _offset: usize) -> SysResult<usize> {
        Ok(buf.len())
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_size = 0;
        kstat.st_mode = self.mode()?.bits() as u32;
        kstat.st_nlink = 1;
        kstat.st_gid = 0;
        kstat.st_uid = 0;
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits_truncate(Mode::S_IFCHR.bits() as u32 | 0o666))
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}
