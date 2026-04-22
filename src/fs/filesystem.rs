use alloc::sync::Arc;
use core::option::Option;

use crate::driver::BlockDriverOps;
use crate::fs::Mode;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::{Statfs, StatfsFlags};

use super::InodeOps;

#[derive(Debug, Clone, Copy, Default)]
pub struct MountOptions {
    pub read_only: bool,
}

impl MountOptions {
    pub const fn new(read_only: bool) -> Self {
        Self { read_only }
    }
}

pub trait FileSystemOps: Send + Sync {
    fn create(
        &self,
        fsno: u32,
        driver: Option<Arc<dyn BlockDriverOps>>,
        options: MountOptions,
    ) -> SysResult<Arc<dyn SuperBlockOps>>;
}

pub trait SuperBlockOps: Send + Sync {
    fn get_root_ino(&self) -> u32;

    fn get_inode(&self, ino: u32) -> SysResult<Arc<dyn InodeOps>>;

    fn create_temp(&self, _mode: Mode) -> SysResult<Arc<dyn InodeOps>> {
        Err(Errno::EOPNOTSUPP)
    }

    fn unmount(&self) -> SysResult<()> {
        // Default implementation does nothing,
        // can be overridden by specific filesystems
        Ok(())
    }

    fn statfs(&self) -> SysResult<Statfs> {
        Err(Errno::EOPNOTSUPP)
    }

    fn sync(&self) -> SysResult<()> {
        // Default implementation does nothing, can be overridden by specific filesystems
        Ok(())
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn statfs_flags(&self) -> StatfsFlags {
        if self.is_readonly() {
            StatfsFlags::ST_RDONLY
        } else {
            StatfsFlags::empty()
        }
    }

    fn type_name(&self) -> &'static str;
}
