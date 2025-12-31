use alloc::sync::Arc;

use crate::driver::{BlockDriverOps, CharDriverOps};
use crate::fs::file::{CharFile, File, FileFlags, FileOps};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;

pub struct CharDevInode {
    ino: u32,
    driver: Arc<dyn CharDriverOps>
}

impl CharDevInode {
    pub fn new(ino: u32, driver: Arc<dyn CharDriverOps>) -> Self {
        Self { ino, driver }
    }
}

impl InodeOps for CharDevInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        // self.driver.read(buf)
        unreachable!()
    }

    fn writeat(&self, buf: &[u8], _offset: usize) -> SysResult<usize> {
        self.driver.write(buf)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_mode = self.mode()?.bits();
        kstat.st_nlink = 1;
        kstat.st_uid = 0;
        kstat.st_gid = 0;
        kstat.st_size = 0;
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits(Mode::S_IFCHR.bits() | 0o666).unwrap())
    }

    fn size(&self) -> SysResult<u64> {
        Ok(0)
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(CharFile::new(self.driver.clone(), self, dentry, flags))
    }
}

pub struct BlockDevInode {
    ino: u32,
    driver: Arc<dyn BlockDriverOps>
}

impl BlockDevInode {
    pub fn new(ino: u32, driver: Arc<dyn BlockDriverOps>) -> Self {
        Self { ino, driver }
    }
}

impl InodeOps for BlockDevInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn readat(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        self.driver.read_at(offset, buf)
            .map(|_| buf.len())
            .map_err(|_| Errno::EIO)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        self.driver.write_at(offset, buf)
            .map(|_| buf.len())
            .map_err(|_| Errno::EIO)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.ino as u64;
        kstat.st_mode = Mode::S_IFBLK.bits() | 0o660;
        kstat.st_nlink = 1;
        kstat.st_uid = 0;
        kstat.st_gid = 0;
        kstat.st_size = self.size()? as i64;
        Ok(kstat)
    }

    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::from_bits(Mode::S_IFBLK.bits() | 0o666).unwrap())
    }

    fn size(&self) -> SysResult<u64> {
        Ok(self.driver.get_block_size() as u64 * self.driver.get_block_count())
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }
    
    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps> {
        Arc::new(File::new(self, dentry.unwrap(), flags))
    }
}
