use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use crate::kernel::errno::SysResult;

use super::superblock::SuperBlock;

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(
        &self,
        sno: u32,
        _driver: Option<Arc<dyn BlockDriverOps>>,
    ) -> SysResult<Arc<dyn SuperBlockOps>> {
        Ok(Arc::new(SuperBlock::new(sno)))
    }
}
