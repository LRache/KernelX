use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, VfsSuperBlock, VfsSuperBlockOps};
use crate::kernel::errno::{Errno, SysResult};

use super::superblock::SuperBlock;

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(
        &self,
        _fsno: u32,
        driver: Option<Arc<dyn BlockDriverOps>>,
        _options: MountOptions,
    ) -> SysResult<Arc<dyn VfsSuperBlockOps>> {
        let driver = driver.ok_or(Errno::ENODEV)?;
        Ok(VfsSuperBlock::new(SuperBlock::new(driver)?))
    }
}
