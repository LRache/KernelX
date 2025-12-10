use alloc::sync::Arc;
use spin::Mutex;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::FileStat;
use crate::fs::file::DirResult;
use crate::fs::InodeOps;
use crate::fs::vfs::Dentry;

use super::{FileOps, SeekWhence};

#[derive(Clone, Copy)]
pub struct FileFlags {
    pub readable: bool,
    pub writable: bool,
    pub blocked: bool
}

impl FileFlags {
    pub const fn dontcare() -> Self {
        FileFlags { readable: true, writable: true, blocked: true }
    }

    pub const fn readonly() -> Self {
        FileFlags { readable: true, writable: false, blocked: true }
    }
}

pub struct File {
    inode: Arc<dyn InodeOps>,
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

    pub fn new_inode(inode: Arc<dyn InodeOps>, dentry: Arc<Dentry>, flags: FileFlags) -> Self {
        File {
            inode,
            dentry,
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

    pub fn ftruncate(&self, new_size: u64) -> SysResult<()> {
        self.inode.truncate(new_size)
    }

    /// Return the dirent and the old file pos.
    pub fn get_dent(&self) -> SysResult<Option<(DirResult, usize)>> {
        let mut pos = self.pos.lock();
        let old_pos = *pos;
        let (dent, next_pos) = match self.inode.get_dent(*pos)? {
            Some(d) => d,
            None => return Ok(None),
        };
        *pos = next_pos;
        
        Ok(Some((dent, old_pos)))
    }
}

impl FileOps for File {
    fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let mut pos = self.pos.lock();
        let len = self.inode.readat(buf, *pos)?;
        *pos += len;
        
        Ok(len)
    }

    fn pread(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let len = self.inode.readat(buf, offset)?;
        Ok(len)
    }

    fn write(&self, buf: &[u8]) -> Result<usize, Errno> {
        let mut pos = self.pos.lock();
        let len = self.inode.writeat(buf, *pos)?;
        *pos += len;
        
        Ok(len)
    }

    fn pwrite(&self, buf: &[u8], offset: usize) -> SysResult<usize> {
        let len = self.inode.writeat(buf, offset)?;
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
        let new_pos;
        match whence {
            SeekWhence::BEG => {
                if offset < 0 {
                    return Err(Errno::EINVAL);
                }
                new_pos = offset;
            }
            SeekWhence::CUR => {
                if offset < 0 && (*pos as isize + offset) < 0 {
                    return Err(Errno::EINVAL);
                }
                new_pos = *pos as isize + offset;
            }
            SeekWhence::END => {
                let size = self.inode.size()?;
                if offset > 0 && (size as isize + offset) < 0 {
                    return Err(Errno::EINVAL);
                }
                new_pos = size as isize + offset;
            }
        }
        if new_pos < 0 {
            return Err(Errno::EINVAL);
        }
        *pos = new_pos as usize;
        
        Ok(*pos)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        self.inode.fstat()
    }

    fn fsync(&self) -> SysResult<()> {
        self.inode.sync()
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        Some(&self.inode)
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        Some(&self.dentry)
    }
}
