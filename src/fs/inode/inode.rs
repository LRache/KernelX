use alloc::string::String;
use alloc::sync::Arc;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::file::DirResult;
use crate::kernel::uapi::FileStat;

use super::{Mode, FileType};

pub trait InodeOps: DowncastSync {
    fn get_ino(&self) -> u32;

    fn get_sno(&self) -> u32;

    fn type_name(&self) -> &'static str;

    fn create(&self, _name: &str, _mode: Mode) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn unlink(&self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> Result<usize, Errno> {
        unimplemented!()
    }
    
    fn writeat(&self, _buf: &[u8], _offset: usize) -> Result<usize, Errno> {
        unimplemented!()
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<DirResult>> {
        Err(Errno::ENOSYS)
    }

    fn lookup(&self, _name: &str) -> SysResult<u32> {
        Err(Errno::ENOTDIR)
    }

    fn rename(&self, _old_name: &str, _new_parent: &Arc<dyn InodeOps>, _new_name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn size(&self) -> SysResult<u64> {
        unimplemented!("{}", self.type_name())
    }
    
    fn mode(&self) -> Mode {
        Mode::empty()
    }

    fn inode_type(&self) -> FileType {
        self.mode().into()
    }

    fn readlink(&self) -> SysResult<String> {
        Err(Errno::EINVAL)
    }

    fn sync(&self) -> SysResult<()> {
        Ok(())
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_size = self.size()? as i64;
        kstat.st_mode = self.mode().bits() as u32;

        Ok(kstat)
    }

    fn truncate(&self, _new_size: u64) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn update_atime(&self, atime: u64, atime_nsec: u64) -> SysResult<()> {
        let _ = atime;
        let _ = atime_nsec;
        Ok(())
    }

    fn update_mtime(&self, mtime: u64, mtime_nsec: u64) -> SysResult<()> {
        let _ = mtime;
        let _ = mtime_nsec;
        Ok(())
    }

    fn update_ctime(&self, ctime: u64, ctime_nsec: u64) -> SysResult<()> {
        let _ = ctime;
        let _ = ctime_nsec;
        Ok(())
    }
}

impl_downcast!(sync InodeOps);
