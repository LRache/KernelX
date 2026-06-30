use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, VfsSuperBlockOps};
use crate::kernel::errno::{Errno, SysResult};

struct SuperBlockEntry {
    superblock: Arc<dyn VfsSuperBlockOps>,
    options: MountOptions,
}

pub struct SuperBlockTable {
    table: Vec<Option<SuperBlockEntry>>,
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
        self.table.push(Some(SuperBlockEntry { superblock, options }));
        Ok(sno as u32)
    }

    pub fn get(&self, sno: u32) -> Option<Arc<dyn VfsSuperBlockOps>> {
        let entry = self.table.get(sno as usize)?.as_ref()?;
        Some(entry.superblock.clone())
    }

    pub fn remount(&mut self, sno: u32, options: MountOptions) -> SysResult<()> {
        let entry = self
            .table
            .get_mut(sno as usize)
            .and_then(|entry| entry.as_mut())
            .ok_or(Errno::EINVAL)?;

        if options.read_only && !entry.options.read_only {
            entry.superblock.sync()?;
        }
        entry.options = options;
        Ok(())
    }

    pub fn is_readonly(&self, sno: u32) -> SysResult<bool> {
        let entry = self
            .table
            .get(sno as usize)
            .and_then(|entry| entry.as_ref())
            .ok_or(Errno::EINVAL)?;

        Ok(entry.options.read_only || entry.superblock.is_readonly())
    }

    pub fn unmount(&mut self, sno: u32) -> SysResult<()> {
        let entry = self
            .table
            .get(sno as usize)
            .and_then(|entry| entry.as_ref())
            .ok_or(crate::kernel::errno::Errno::EINVAL)?;
        let sb = entry.superblock.clone();
        sb.sync()?;
        sb.unmount()?;
        *self
            .table
            .get_mut(sno as usize)
            .ok_or(crate::kernel::errno::Errno::EINVAL)? = None;
        Ok(())
    }

    pub fn unmount_all(&mut self) -> SysResult<()> {
        for entry in self.table.iter().flatten() {
            entry.superblock.sync()?;
            entry.superblock.unmount()?;
        }
        self.table.clear();
        Ok(())
    }

    pub fn sync_all(&self) -> SysResult<()> {
        for entry in self.table.iter().flatten() {
            entry.superblock.sync()?;
        }
        Ok(())
    }
}
