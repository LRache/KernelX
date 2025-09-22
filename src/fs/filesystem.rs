use alloc::sync::Arc;
use alloc::boxed::Box;
use core::option::Option;

use crate::kernel::errno::SysResult;
use crate::fs::inode::Inode;
use crate::driver::block::BlockDriver;

pub trait FileSystem: Send + Sync {
    fn create(&self, fsno: u32, driver: Option<Box<dyn BlockDriver>>) -> SysResult<Arc<dyn SuperBlock>>;
}

pub trait SuperBlock: Send + Sync {
    fn get_root_ino(&self) -> u32 {
        panic!("get_root_inode not implemented for this filesystem");
    }

    fn get_inode(&self, _ino: u32) -> SysResult<Box<dyn Inode>> {
        // Default implementation returns None, can be overridden by specific filesystems
        unimplemented!();
    }

    fn unmount(&self) -> SysResult<()> {
        // Default implementation does nothing, can be overridden by specific filesystems
        Ok(())
    }
}
