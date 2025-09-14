use alloc::boxed::Box;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode;
use crate::fs::FileStat;

pub trait Inode: Send + Sync {
    fn get_ino(&self) -> u32;

    fn get_sno(&self) -> u32;

    fn readat(&mut self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        panic!("readat not implemented for this inode type")
    }
    
    fn writeat(&mut self, _buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        panic!("writeat not implemented for this inode type")
    }

    fn lookup(&mut self, _name: &str) -> Result<u32, Errno> {
        panic!("lookup not implemented for this inode type")
    }

    fn size(&self) -> Result<usize, Errno> {
        panic!("size not implemented for this inode type")
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::new();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_size = self.size()? as i64;
        
        Ok(kstat)
    }
}

pub struct LockedInode {
    index: inode::Index,
    inner: Mutex<Box<dyn Inode>>,
}

impl LockedInode {
    pub const fn new(index: &inode::Index, inode: Box<dyn Inode>) -> Self {
        Self {
            index: *index,
            inner: Mutex::new(inode),
        }
    }

    pub fn get_index(&self) -> inode::Index {
        self.index
    }

    pub fn get_sno(&self) -> u32 {
        self.index.sno
    }

    pub fn get_ino(&self) -> u32 {
        self.index.ino
    }

    pub fn readat(&self, buf: &mut [u8], offset: usize) -> Result<usize, Errno> {
        self.inner.lock().readat(buf, offset)
    }

    pub fn writeat(&self, buf: &[u8], offset: usize) -> Result<usize, Errno> {
        self.inner.lock().writeat(buf, offset)
    }

    pub fn lookup(&self, name: &str) -> Result<u32, Errno> {
        self.inner.lock().lookup(name)
    }

    pub fn size(&self) -> Result<usize, Errno> {
        self.inner.lock().size()
    }

    pub fn fstat(&self) -> SysResult<FileStat> {
        self.inner.lock().fstat()
    }
}
