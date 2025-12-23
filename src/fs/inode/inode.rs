use core::time::Duration;
use alloc::sync::Arc;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::fs::Dentry;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{FileStat, Uid};
use crate::fs::file::{DirResult, FileFlags, FileOps};

use super::{Mode, FileType};

pub trait InodeOps: DowncastSync {
    fn get_ino(&self) -> u32;

    fn type_name(&self) -> &'static str;

    fn create(&self, _name: &str, _mode: Mode) -> SysResult<Arc<dyn InodeOps>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn link(&self, name: &str, target: &Arc<dyn InodeOps>) -> SysResult<()> {
        let _ = name;
        let _ = target;
        Err(Errno::EOPNOTSUPP)
    }

    fn unlink(&self, _name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn symlink(&self, target: &str) -> SysResult<()> {
        let _ = target;
        Err(Errno::EOPNOTSUPP)
    }

    fn readat(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        unimplemented!()
    }
    
    fn writeat(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
        unimplemented!()
    }

    fn get_dent(&self, _index: usize) -> SysResult<Option<(DirResult, usize)>> {
        Err(Errno::ENOTDIR)
    }

    fn lookup(&self, _name: &str) -> SysResult<u32> {
        Err(Errno::ENOTDIR)
    }

    fn rename(&self, _old_name: &str, _new_parent: &Arc<dyn InodeOps>, _new_name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn readlink(&self, buf: &mut [u8]) -> SysResult<Option<usize>> {
        let _ = buf;
        Ok(None)
    }

    fn size(&self) -> SysResult<u64>;
    
    fn mode(&self) -> SysResult<Mode> {
        Ok(Mode::empty())
    }

    fn chmod(&self, _mode: Mode) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }
      
    fn owner(&self) -> SysResult<(Uid, Uid)> {
        Ok((0, 0))
    }

    fn chown(&self, uid: Option<Uid>, gid: Option<Uid>) -> SysResult<()> {
        let _ = uid;
        let _ = gid;
        Err(Errno::EOPNOTSUPP)
    }

    fn inode_type(&self) -> SysResult<FileType> {
        self.mode().map(|inode| inode.into())
    }

    fn sync(&self) -> SysResult<()> {
        Ok(())
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::default();
        kstat.st_ino = self.get_ino() as u64;
        kstat.st_size = self.size()? as i64;
        kstat.st_mode = self.mode()?.bits() as u32;

        Ok(kstat)
    }

    fn truncate(&self, _new_size: u64) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn update_atime(&self, time: &Duration) -> SysResult<()> {
        let _ = time;
        Ok(())
    }

    fn update_mtime(&self, time: &Duration) -> SysResult<()> {
        let _ = time;
        Ok(())
    }

    fn update_ctime(&self, time: &Duration) -> SysResult<()> {
        let _ = time;
        Ok(())
    }

    fn wrap_file(self: Arc<Self>, dentry: Option<Arc<Dentry>>, flags: FileFlags) -> Arc<dyn FileOps>;
}

impl_downcast!(sync InodeOps);

pub type Inode = Arc<dyn InodeOps>;
