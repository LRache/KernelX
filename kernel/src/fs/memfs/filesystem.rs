use alloc::sync::Arc;
use alloc::boxed::Box;

use crate::kernel::errno::Errno;
use crate::fs::filesystem::{FileSystem, SuperBlock};
use crate::driver::block::BlockDriver;

use super::superblock::MemoryFileSystemSuperBlock;

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
    fn create(&self, sno: u32, _driver: Option<Box<dyn BlockDriver>>) -> Result<Arc<dyn SuperBlock>, Errno> {
        Ok(MemoryFileSystemSuperBlock::new(sno))
    }
}
