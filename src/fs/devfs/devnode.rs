use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::driver::{BlockDriverOps, CharDriverOps};
use crate::fs::file::{CharFile, FileFlags, FileOps, RandomAccessFile};
use crate::fs::inode::InodeLockState;
use crate::fs::{Dentry, Inode, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::AddrSpace;
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;

pub struct CharDevInode {
    ino: u32,
    driver: Arc<dyn CharDriverOps>,
    lock_state: SpinLock<InodeLockState>,
}

impl CharDevInode {
    pub fn new(ino: u32, driver: Arc<dyn CharDriverOps>) -> Self {
        Self {
            ino,
            driver,
            lock_state: SpinLock::new(InodeLockState::new(), "CharDevInode::lock_state"),
        }
    }

    pub fn driver(&self) -> &Arc<dyn CharDriverOps> {
        &self.driver
    }
}

impl InodeOps for CharDevInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize, _direct: bool) -> SysResult<usize> {
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

    fn wrap_file(
        self: Arc<Self>,
        inode: Arc<Inode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> Arc<dyn FileOps> {
        Arc::new(CharFile::new(self.driver.clone(), inode, dentry, flags))
    }
}

pub struct BlockDevInode {
    ino: u32,
    driver: Arc<dyn BlockDriverOps>,
    lock_state: SpinLock<InodeLockState>,
}

impl BlockDevInode {
    pub fn new(ino: u32, driver: Arc<dyn BlockDriverOps>) -> Self {
        Self {
            ino,
            driver,
            lock_state: SpinLock::new(InodeLockState::new(), "BlockDevInode::lock_state"),
        }
    }

    pub fn driver(&self) -> &Arc<dyn BlockDriverOps> {
        &self.driver
    }
}

impl InodeOps for BlockDevInode {
    fn get_ino(&self) -> u32 {
        self.ino
    }

    fn lock_state(&self) -> Option<&SpinLock<InodeLockState>> {
        Some(&self.lock_state)
    }

    fn readat(&self, buf: &mut [u8], offset: usize, _direct: bool) -> SysResult<usize> {
        self.driver
            .read_at(offset, buf)
            .map(|_| buf.len())
            .map_err(|_| Errno::EIO)
    }

    fn writeat(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        if self.driver.is_readonly() {
            return Err(Errno::EROFS);
        }

        self.driver
            .write_at(offset, buf)
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

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        #[derive(TryFromPrimitive)]
        #[allow(non_camel_case_types)]
        #[repr(u32)]
        enum Request {
            BLKRASET = 0x1262,
            BLKRAGET = 0x1263,
        }

        let request = Request::try_from_primitive(request as u32).map_err(|_| Errno::ENOTTY)?;
        match request {
            Request::BLKRASET => {
                self.driver.set_readahead(arg);
                Ok(0)
            }
            Request::BLKRAGET => {
                addrspace.copy_to_user(arg, self.driver.get_readahead())?;
                Ok(0)
            }
        }
    }

    fn type_name(&self) -> &'static str {
        "devfs"
    }

    fn wrap_file(
        self: Arc<Self>,
        inode: Arc<Inode>,
        dentry: Option<Arc<Dentry>>,
        flags: FileFlags,
    ) -> Arc<dyn FileOps> {
        Arc::new(RandomAccessFile::new(inode, dentry.unwrap(), flags))
    }
}
