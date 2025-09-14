use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::kernel::errno::SysResult;
use crate::fs::filesystem::{FileSystem, SuperBlock};
use crate::driver::block::BlockDriver;

pub struct SuperBlockTable {
    table: Vec<Option<Arc<dyn SuperBlock>>>
}

impl SuperBlockTable {
    pub const fn new() -> Self {
        SuperBlockTable {
            table: Vec::new(),
        }
    }

    pub fn alloc(&mut self, fs: &Box<dyn FileSystem>, driver: Option<Box<dyn BlockDriver>>) -> SysResult<u32> {
        let sno = self.table.len();
        let superblock = fs.create(sno as u32, driver)?;
        self.table.push(Some(superblock));
        Ok(sno as u32)
    }

    pub fn get(&self, sno: u32) -> Option<Arc<dyn SuperBlock>> {
        let fs = self.table.get(sno as usize)?;
        fs.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}