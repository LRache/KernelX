use alloc::sync::Arc;

use crate::fs::file::DirResult;
use crate::fs::vfs::Dentry;
use crate::fs::{InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::ubuf::UAddrSpaceBuffer;
use crate::kernel::uapi::FileStat;
use crate::klib::SleepLock;

use super::{FileOps, SeekWhence};

#[derive(Clone, Copy)]
pub struct FileFlags {
    pub readable: bool,
    pub writable: bool,
    pub blocked: bool,
    pub append: bool,
    pub direct: bool,
}

impl FileFlags {
    pub const fn dontcare() -> Self {
        FileFlags {
            readable: true,
            writable: true,
            blocked: true,
            append: false,
            direct: false,
        }
    }

    pub const fn readonly() -> Self {
        FileFlags {
            readable: true,
            writable: false,
            blocked: true,
            append: false,
            direct: false,
        }
    }
}

pub struct File {
    inode: Arc<dyn InodeOps>,
    dentry: Arc<Dentry>,
    pos: SleepLock<usize>,

    pub flags: FileFlags,
}

impl File {
    pub fn new(inode: Arc<dyn InodeOps>, dentry: Arc<Dentry>, flags: FileFlags) -> Self {
        Self {
            inode,
            dentry,
            pos: SleepLock::new(0, "File::pos"),
            flags,
        }
    }

    pub fn read_at(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let len = self.inode.readat(buf, offset, self.flags.direct)?;
        Ok(len)
    }

    pub fn ftruncate(&self, new_size: u64) -> SysResult<()> {
        self.inode.truncate(new_size)
    }

    /// Return the dirent and the old file pos.
    pub fn get_dent(&self) -> SysResult<Option<(DirResult, usize)>> {
        let mut pos = self.pos.lock();
        let old_pos = *pos;
        let (mut dent, next_pos) = match self.inode.get_dent(*pos)? {
            Some(d) => d,
            None => return Ok(None),
        };
        *pos = next_pos;

        if dent.name == ".." {
            if let Some(parent) = self.dentry.get_parent() {
                dent.ino = parent.get_inode().get_ino();
            }
        }

        Ok(Some((dent, old_pos)))
    }

    pub fn mode(&self) -> SysResult<Mode> {
        self.inode.mode()
    }

    pub fn owner(&self) -> SysResult<(u32, u32)> {
        self.inode.owner()
    }
}

impl FileOps for File {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        let len = self.inode.readat(buf, *pos, self.flags.direct)?;
        *pos += len;

        Ok(len)
    }

    fn pread(&self, buf: &mut [u8], offset: usize) -> SysResult<usize> {
        let len = self.inode.readat(buf, offset, self.flags.direct)?;
        Ok(len)
    }

    fn read_to_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        let len = self.inode.read_to_user(ubuf, *pos, self.flags.direct)?;
        *pos += len;
        Ok(len)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        if self.flags.append {
            let size = self.inode.size()?;

            *pos = size as usize;
        }
        let len = self.inode.writeat(buf, *pos)?;
        *pos += len;

        Ok(len)
    }

    fn pwrite(&self, buf: &[u8], mut offset: usize) -> SysResult<usize> {
        if self.flags.append {
            offset = self.inode.size()? as usize;
        }
        let len = self.inode.writeat(buf, offset)?;
        Ok(len)
    }

    fn write_from_user(&self, ubuf: &UAddrSpaceBuffer) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        if self.flags.append {
            *pos = self.inode.size()? as usize;
        }
        let len = self.inode.write_from_user(ubuf, *pos, self.flags.direct)?;
        *pos += len;
        Ok(len)
    }

    fn flags(&self) -> FileFlags {
        self.flags
    }

    fn seek(&self, offset: isize, whence: SeekWhence) -> SysResult<usize> {
        let mut pos = self.pos.lock();
        let new_pos = match whence {
            SeekWhence::BEG => {
                if offset < 0 {
                    return Err(Errno::EINVAL);
                }
                offset as isize
            }
            SeekWhence::CUR => {
                if offset < 0 && (*pos as isize + offset) < 0 {
                    return Err(Errno::EINVAL);
                }
                *pos as isize + offset
            }
            SeekWhence::END => {
                let size = self.inode.size()?;
                if offset > 0 && (size as isize + offset) < 0 {
                    return Err(Errno::EINVAL);
                }
                size as isize + offset
            }
        };

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
