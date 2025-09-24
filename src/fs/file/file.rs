use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::file::DirResult;
use crate::kernel::errno::{Errno, SysResult};
use crate::fs::inode::LockedInode;
use crate::fs::vfs::Dentry;
use crate::ktrace;

use super::FileStat;

pub enum SeekWhence {
    BEG,
    CUR,
    END,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    FIFO,
    Socket,
    Unknown,
}

#[derive(Clone, Copy)]
pub struct FileFlags {
    pub writable: bool,
    pub cloexec: bool
}

impl FileFlags {
    pub const fn dontcare() -> Self {
        FileFlags { writable: false, cloexec: false }
    }
}

pub struct File {
    inode: Arc<LockedInode>,
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

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let mut pos = self.pos.lock();
        let len = self.inode.readat(buf, *pos)?;
        *pos += len;
        
        Ok(len)
    }

    pub fn read_at(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let len = self.inode.readat(buf, offset)?;
        Ok(len)
    }

    pub fn write(&self, buf: &[u8]) -> Result<usize, Errno> {
        let mut pos = self.pos.lock();
        let len = self.inode.writeat(buf, *pos)?;
        *pos += len;
        
        Ok(len)
    }

    pub fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize> {
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

    pub fn close(&self) -> Result<(), Errno> {
        // Placeholder for close logic, if needed
        Ok(())
    }

    pub fn ioctl(&self, _request: usize, _arg: usize) -> SysResult<usize> {
        Err(Errno::ENOSYS) // Placeholder for unimplemented ioctl commands
    }

    pub fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::new();
        ktrace!("fstat: inode type={}", self.inode.type_name());
        kstat.st_ino = self.inode.get_ino() as u64;
        kstat.st_size = self.inode.size()? as i64;
        kstat.st_mode = self.inode.mode().bits() as u32;
        
        Ok(kstat)
    }

    pub fn get_dent(&self) -> SysResult<Option<DirResult>> {
        let mut pos = self.pos.lock();
        let dent = match self.inode.get_dent(*pos)? {
            Some(d) => d,
            None => return Ok(None),
        };
        *pos += 1;
        
        Ok(Some(dent))
    }

    pub fn get_inode(&self) -> &Arc<LockedInode> {
        &self.inode
    }

    pub fn get_dentry(&self) -> &Arc<Dentry> {
        &self.dentry
    }
}
