use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::kernel::errno::SysResult;
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use crate::driver::BlockDriverOps;

pub struct SuperBlockTable {
    table: Vec<Option<Arc<dyn SuperBlockOps>>>
}

impl SuperBlockTable {
    pub const fn new() -> Self {
        SuperBlockTable {
            table: Vec::new(),
        }
    }

    pub fn alloc(&mut self, fs: &'static dyn FileSystemOps, driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<u32> {
        let sno = self.table.len();
        let superblock = fs.create(sno as u32, driver)?;
        self.table.push(Some(superblock));
        Ok(sno as u32)
    }

    pub fn get(&self, sno: u32) -> Option<Arc<dyn SuperBlockOps>> {
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

    pub fn sync_all(&self) -> SysResult<()> {
        for fs in &self.table {
            if let Some(sb) = fs {
                sb.sync();
            }
        }
        Ok(())
    }
}