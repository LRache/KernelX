use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::kernel::errno::SysResult;
use crate::fs::filesystem::{FileSystem, SuperBlock};
use crate::driver::BlockDriverOps;

pub struct SuperBlockTable {
    table: Vec<Option<Arc<dyn SuperBlock>>>
}

impl SuperBlockTable {
    pub const fn new() -> Self {
        SuperBlockTable {
            table: Vec::new(),
        }
    }

    pub fn alloc(&mut self, fs: &Box<dyn FileSystem>, driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<u32> {
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

    pub fn unmount_all(&self) -> SysResult<()> {
        for fs in &self.table {
            if let Some(sb) = fs {
                sb.unmount()?;
            }
        }
        Ok(())
    }
}