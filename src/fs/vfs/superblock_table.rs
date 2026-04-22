use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps};
use crate::kernel::errno::SysResult;

pub struct SuperBlockTable {
    table: Vec<Option<Arc<dyn SuperBlockOps>>>,
}

impl SuperBlockTable {
    pub const fn new() -> Self {
        SuperBlockTable { table: Vec::new() }
    }

    pub fn mount(
        &mut self,
        fs: &'static dyn FileSystemOps,
        driver: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> SysResult<u32> {
        let sno = self.table.len();
        let superblock = fs.create(sno as u32, driver, options)?;
        self.table.push(Some(superblock));
        Ok(sno as u32)
    }

    pub fn get(&self, sno: u32) -> Option<Arc<dyn SuperBlockOps>> {
        let fs = self.table.get(sno as usize)?;
        fs.clone()
    }

    pub fn unmount(&mut self, sno: u32) -> SysResult<()> {
        let sb = self
            .table
            .get(sno as usize)
            .and_then(|entry| entry.clone())
            .ok_or(crate::kernel::errno::Errno::EINVAL)?;
        sb.sync()?;
        sb.unmount()?;
        *self
            .table
            .get_mut(sno as usize)
            .ok_or(crate::kernel::errno::Errno::EINVAL)? = None;
        Ok(())
    }

    pub fn unmount_all(&mut self) -> SysResult<()> {
        for sb in self.table.iter().flatten() {
            sb.sync()?;
            sb.unmount()?;
        }
        self.table.clear();
        Ok(())
    }

    pub fn sync_all(&self) -> SysResult<()> {
        for sb in self.table.iter().flatten() {
            sb.sync()?;
        }
        Ok(())
    }
}
