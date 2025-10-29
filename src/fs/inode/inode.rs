use alloc::string::String;
use alloc::sync::Arc;
use downcast_rs::{DowncastSync, impl_downcast};

use crate::kernel::errno::{Errno, SysResult};
use crate::fs::file::DirResult;

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
        Err(Errno::ENOENT)
    }

    fn rename(&self, _old_name: &str, _new_parent: &Arc<dyn InodeOps>, _new_name: &str) -> SysResult<()> {
        Err(Errno::EOPNOTSUPP)
    }

    fn size(&self) -> SysResult<u64> {
        unimplemented!()
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
}

impl_downcast!(sync InodeOps);
