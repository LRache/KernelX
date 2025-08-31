use alloc::sync::Arc;
use alloc::boxed::Box;

use super::superblock::MemoryFileSystemSuperBlock;
use crate::driver::block::BlockDevice;
use crate::fs::filesystem::{FileSystem, SuperBlock};
use crate::kernel::errno::Errno;

pub struct MemoryFileSystem;

impl MemoryFileSystem {
    pub fn new() -> Self {
        MemoryFileSystem {}
    }
}

unsafe extern "C" {
    static __bin_resource_start: u8;
    static __bin_resource_end  : u8;
}

impl FileSystem for MemoryFileSystem {
    fn create(&self, fsno: usize, _device: Option<Box<dyn BlockDevice>>) -> Result<Arc<dyn SuperBlock>, Errno> {
        Ok(MemoryFileSystemSuperBlock::new(fsno))
    }
}
