use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, SuperBlockOps};
use crate::fs::memtreefs;
use crate::kernel::errno::SysResult;

struct TmpfsInfo;
impl memtreefs::StaticFsInfo for TmpfsInfo {
    fn type_name() -> &'static str {
        "tmpfs"
    }
}

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(&self, sno: u32, _driver: Option<Arc<dyn BlockDriverOps>>) -> SysResult<Arc<dyn SuperBlockOps>> {
        Ok(Arc::new(memtreefs::SuperBlock::<TmpfsInfo>::new(sno)))
    }
}
