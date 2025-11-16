use alloc::sync::Arc;
use alloc::boxed::Box;
use core::option::Option;

use crate::fs::Mode;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::Statfs;
use crate::driver::BlockDriverOps;

use super::InodeOps;

pub trait FileSystemOps: Send + Sync {
    fn create(&self, fsno: u32, driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<Arc<dyn SuperBlockOps>>;
}

pub trait SuperBlockOps: Send + Sync {
    fn get_root_ino(&self) -> u32;

    fn get_inode(&self, ino: u32) -> SysResult<Box<dyn InodeOps>>;

    fn create_temp(&self, _mode: Mode) -> SysResult<Box<dyn InodeOps>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn unmount(&self) -> SysResult<()> {
        // Default implementation does nothing, can be overridden by specific filesystems
        Ok(())
    }

    fn statfs(&self) -> SysResult<Statfs> {
        Err(Errno::EOPNOTSUPP)
    }
}
