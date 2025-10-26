use alloc::sync::Arc;
use alloc::boxed::Box;
use core::option::Option;

use crate::kernel::errno::SysResult;
use crate::fs::inode::Inode;
use crate::driver::BlockDriverOps;

pub trait FileSystem: Send + Sync {
    fn create(&self, fsno: u32, driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<Arc<dyn SuperBlock>>;
}

pub trait SuperBlock: Send + Sync {
    fn get_root_ino(&self) -> u32;

    fn get_inode(&self, ino: u32) -> SysResult<Box<dyn Inode>>;

    fn unmount(&self) -> SysResult<()> {
        // Default implementation does nothing, can be overridden by specific filesystems
        Ok(())
    }
}
