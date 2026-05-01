use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps};
use crate::fs::memtreefs;
use crate::kernel::errno::SysResult;

struct TmpfsInfo;
impl memtreefs::StaticFsInfo for TmpfsInfo {
    const MAX_FILENAME_LEN: Option<usize> = Some(255);

    fn type_name() -> &'static str {
        "tmpfs"
    }

    fn statfs_magic() -> u64 {
        0x01021994
    }
}

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(
        &self,
        _sno: u32,
        _driver: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> SysResult<Arc<dyn SuperBlockOps>> {
        Ok(Arc::new(memtreefs::SuperBlock::<TmpfsInfo>::new(options.read_only)))
    }
}
