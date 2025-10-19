use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;
use crate::fs::file::DirResult;
use crate::fs::inode::Inode;
use crate::fs::vfs::Dentry;

use super::{FileOps, SeekWhence};

#[derive(Clone, Copy)]
pub struct FileFlags {
    pub readable: bool,
    pub writable: bool
}

impl FileFlags {
    pub const fn dontcare() -> Self {
        FileFlags { readable: true, writable: true }
    }
}

pub struct File {
    inode: Arc<dyn Inode>,
    dentry: Arc<Dentry>,
    pos: Mutex<usize>, 
    
    pub flags: FileFlags,
}

impl File {
    pub fn new(dentry: &Arc<Dentry>, flags: FileFlags) -> Self {
        File {
            inode: dentry.get_inode().clone(),
            dentry: dentry.clone(),
            pos: Mutex::new(0),
            flags
        }
    }

    pub fn read_at(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let len = self.inode.readat(buf, offset)?;
        Ok(len)
    }

    pub fn write_at(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        if !self.flags.writable {
            return Err(Errno::EPERM);
        }
        let len = self.inode.writeat(buf, offset)?;
        Ok(len)
    }
}

impl FileOps for File {
    fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let mut pos = self.pos.lock();
        let len = self.inode.readat(buf, *pos)?;
        *pos += len;
        
        Ok(len)
    }

    fn write(&self, buf: &[u8]) -> Result<usize, Errno> {
        let mut pos = self.pos.lock();
        let len = self.inode.writeat(buf, *pos)?;
        *pos += len;
        
        Ok(len)
    }

    fn readable(&self) -> bool {
        self.flags.readable
    }

    fn writable(&self) -> bool {
        self.flags.writable
    }

    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        match whence {
            SeekWhence::BEG => {
                if offset < 0 {
                    return Err(Errno::EINVAL);
                }
                *pos = offset as usize;
            }
            SeekWhence::CUR => {
                if offset < 0 && (*pos as isize + offset) < 0 {
                    return Err(Errno::EINVAL);
                }
                *pos = (*pos as isize + offset) as usize;
            }
            SeekWhence::END => {
                let size = self.inode.size()?;
                if offset > 0 && (size as isize + offset) < 0 {
                    return Err(Errno::EINVAL);
                }
                *pos = (size as isize + offset) as usize;
            }
        }
        Ok(*pos)
    }

    fn ioctl(&self, _request: usize, _arg: usize) -> SysResult<usize> {
        Err(Errno::ENOSYS) // Placeholder for unimplemented ioctl commands
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        
        kstat.st_ino = self.inode.get_ino() as u64;
        kstat.st_size = self.inode.size()? as i64;
        kstat.st_mode = self.inode.mode().bits() as u32;
        
        Ok(kstat)
    }

    fn get_dent(&self) -> SysResult<Option<DirResult>> {
        let mut pos = self.pos.lock();
        let dent = match self.inode.get_dent(*pos)? {
            Some(d) => d,
            None => return Ok(None),
        };
        *pos += 1;
        
        Ok(Some(dent))
    }

    fn get_inode(&self) -> Option<&Arc<dyn Inode>> {
        Some(&self.inode)
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        Some(&self.dentry)
    }
}
