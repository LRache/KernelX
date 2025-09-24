use alloc::boxed::Box;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::file::{FileStat, DirResult};

use super::{Mode, Index};

pub trait Inode {
    fn get_ino(&self) -> u32;

    fn get_sno(&self) -> u32;

    fn type_name(&self) -> &'static str;

    fn create(&mut self, _name: &str, _mode: Mode) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readat(&mut self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        unimplemented!()
    }
    
    fn writeat(&mut self, _buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        unimplemented!()
    }

    fn get_dent(&mut self, _index: usize) -> SysResult<Option<DirResult>> {
        Err(Errno::ENOSYS)
    }

    fn lookup(&mut self, _name: &str) -> Result<u32, Errno> {
        Err(Errno::ENOENT)
    }

    fn size(&self) -> Result<usize, Errno> {
        unimplemented!()
    }
    
    fn mode(&self) -> Mode {
        Mode::empty()
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::new();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_size = self.size()? as i64;
        
        Ok(kstat)
    }

    fn destroy(&mut self) -> SysResult<()> {
        Ok(())
    }
}

pub struct LockedInode {
    index: Index,
    inner: Mutex<Box<dyn Inode>>,
}

impl LockedInode {
    pub const fn new(index: &Index, inode: Box<dyn Inode>) -> Self {
        Self {
            index: *index,
            inner: Mutex::new(inode),
        }
    }

    pub fn get_index(&self) -> Index {
        self.index
    }

    pub fn get_sno(&self) -> u32 {
        self.index.sno
    }

    pub fn get_ino(&self) -> u32 {
        self.index.ino
    }

    pub fn create(&self, name: &str, mode: Mode) -> SysResult<()> {
        self.inner.lock().create(name, mode)
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

    pub fn mode(&self) -> Mode {
        self.inner.lock().mode()
    }

    pub fn get_dent(&self, index: usize) -> SysResult<Option<DirResult>> {
        self.inner.lock().get_dent(index)
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
