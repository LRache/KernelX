use alloc::sync::Arc;
use alloc::boxed::Box;
use core::option::Option;

use crate::driver::block::BlockDevice;
use crate::kernel::errno::Errno;
use crate::fs::inode::Inode;

pub trait FileSystem: Send + Sync {
    fn create(&self, fsno: usize, device: Option<Box<dyn BlockDevice>>) -> Result<Arc<dyn SuperBlock>, Errno>;
}

pub trait SuperBlock: Send + Sync {
    fn get_root_inode(&self) -> Box<dyn Inode> {
        panic!("get_root_inode not implemented for this filesystem");
    }

    fn get_inode(&self, _ino: u32) -> Result<Box<dyn Inode>, Errno> {
        // Default implementation returns None, can be overridden by specific filesystems
        unimplemented!();
    }
}
