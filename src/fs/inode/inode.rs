use alloc::boxed::Box;
use spin::Mutex;

use crate::kernel::errno::Errno;

pub trait Inode: Send + Sync {
    fn get_ino(&self) -> u32;

    fn get_fsno(&self) -> usize;

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
}

pub struct LockedInode {
    sno: u32,
    ino: u32,
    inner: Mutex<Box<dyn Inode>>,
}

impl LockedInode {
    pub fn new(sno: u32, ino: u32, inode: Box<dyn Inode>) -> Self {
        Self {
            sno,
            ino,
            inner: Mutex::new(inode),
        }
    }

    pub fn get_sno(&self) -> u32 {
        self.sno
    }

    pub fn get_ino(&self) -> u32 {
        self.ino
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
}
