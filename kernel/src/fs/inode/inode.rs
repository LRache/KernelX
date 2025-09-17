use alloc::boxed::Box;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode;
use crate::fs::FileStat;
use crate::fs::file::FileFlags;

pub trait Inode {
    fn get_ino(&self) -> u32;

    fn get_sno(&self) -> u32;

    fn type_name(&self) -> &'static str;

    fn readat(&mut self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        unimplemented!()
    }
    
    fn writeat(&mut self, _buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        unimplemented!()
    }

    fn lookup(&mut self, _name: &str) -> Result<u32, Errno> {
        Err(Errno::ENOENT)
    }

    fn size(&self) -> Result<usize, Errno> {
        unimplemented!()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::new();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_size = self.size()? as i64;
        
        Ok(kstat)
    }

    fn mkdir(&mut self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn create(&mut self, _name: &str, _flags: FileFlags) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn destroy(&mut self) -> SysResult<()> {
        Ok(())
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

    // pub fn fstat(&self) -> SysResult<FileStat> {
    //     self.inner.lock().fstat()
    // }

    pub fn mkdir(&self, name: &str) -> SysResult<()> {
        self.inner.lock().mkdir(name)
    }

    pub fn create(&self, name: &str, flags: FileFlags) -> SysResult<()> {
        self.inner.lock().create(name, flags)
    }

    pub fn destroy(&self) -> SysResult<()> {
        self.inner.lock().destroy()
    }

    pub fn type_name(&self) -> &'static str {
        self.inner.lock().type_name()
    }
}

unsafe impl Send for LockedInode {}
unsafe impl Sync for LockedInode {}
