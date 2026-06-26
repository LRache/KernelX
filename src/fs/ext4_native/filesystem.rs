use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps};
use crate::kernel::errno::{Errno, SysResult};

use super::superblock::SuperBlock;

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(
        &self,
        _fsno: u32,
        driver: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> SysResult<Arc<dyn SuperBlockOps>> {
        let driver = driver.ok_or(Errno::ENODEV)?;
        Ok(SuperBlock::new(driver, options.read_only)?)
    }
}
