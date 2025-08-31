use alloc::sync::Arc;
use spin::Mutex;

use crate::fs::inode::manager::InodeWrapper;
use crate::kernel::errno::Errno;
use super::FileStat;

pub enum SeekWhence {
    BEG,
    CUR,
    END,
}

pub enum FileType {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Pipe,
    Socket,
}

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
    inode: Arc<InodeWrapper>,
    pos: Mutex<usize>,
    ftype: FileType,
    
    pub flags: FileFlags,
}

impl File {
    pub const fn new(inode: Arc<InodeWrapper>, flags: FileFlags) -> Self {
        File {
            inode,
            pos: Mutex::new(0),
            ftype: FileType::Regular,
            flags
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let mut pos = self.pos.lock();
        let len = self.inode.readat(buf, *pos)?;
        *pos += len;
        
        Ok(len)
    }

    pub fn read_at(&self, buf: &mut [u8], offset: usize) -> Result<usize, Errno> {
        let len = self.inode.readat(buf, offset)?;
        Ok(len)
    }

    pub fn write(&self, buf: &[u8]) -> Result<usize, Errno> {
        let mut pos = self.pos.lock();
        let len = self.inode.writeat(buf, *pos)?;
        *pos += len;
        
        Ok(len)
    }

    pub fn seek(&self, offset: isize, whence: SeekWhence) -> Result<usize, Errno> {
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

    pub fn ioctl(&self, _request: usize, _arg: usize) -> Result<usize, Errno> {
        Err(Errno::ENOSYS) // Placeholder for unimplemented ioctl commands
    }

    pub fn fstat(&self) -> Result<FileStat, Errno> {
        let mut kstat = FileStat::new();
        kstat.st_ino = self.inode.get_ino() as u64;
        kstat.st_size = self.inode.size()? as i64;
        
        Ok(kstat)
    }

    pub fn get_inode(&self) -> &Arc<InodeWrapper> {
        &self.inode
    }
}
