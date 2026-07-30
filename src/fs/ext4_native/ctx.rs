use alloc::sync::Arc;

use crate::driver::BlockDriverOps;
use crate::fs::Mode;
use crate::fs::ext4_native::inode::Inode;
use crate::fs::ext4_native::ondisk::{Ext4IncompatFeatures, Ext4RoCompatFeatures, mount_errno};
use crate::fs::filesystem::{FileSystemOps, MountOptions, SuperBlockOps, VfsSuperBlock, VfsSuperBlockOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::uapi::Statfs;
use crate::klib::SleepRwLockOnStack;

pub struct Context {
    pub(super) fsno: u32,
    pub(super) driver: Arc<dyn BlockDriverOps>,

    // checksum / format identity
    pub(super) uuid: [u8; 16],
    pub(super) hash_seed: [u32; 4],
    pub(super) flags: u32,
    pub(super) checksum_seed: u32,
    pub(super) metadata_csum: bool,

    // filesystem geometry
    pub(super) block_size: u32,
    pub(super) blocks_count: u64,
    pub(super) first_data_block: u32,
    pub(super) blocks_per_group: u32,
    pub(super) inodes_per_group: u32,
    pub(super) inode_size: u16,
    pub(super) desc_size: u16,
    pub(super) groups_count: u32,

    // feature gates used by low-level readers/writers
    pub(super) feature_compat: u32,
    pub(super) feature_incompat: Ext4IncompatFeatures,
    pub(super) feature_ro_compat: Ext4RoCompatFeatures,

    // block-allocation cursor: next search starts near the last allocated group
    pub(super) alloc_hint_group: core::sync::atomic::AtomicU32,
}

pub struct SuperBlock {
    context: Arc<SleepRwLockOnStack<Context>>,
}

impl SuperBlockOps for SuperBlock {
    type Inode = Inode;

    fn get_root_ino(&self) -> u32 {
        2
    }

    fn get_inode(&self, ino: u32) -> SysResult<Self::Inode> {
        let inode = self.context.read().read_inode(ino)?;
        Ok(Inode::new(Arc::downgrade(&self.context), inode))
    }

    fn create_temp(&self, _mode: Mode) -> SysResult<Self::Inode> {
        unimplemented!("ext4_native::SuperBlock::create_temp")
    }

    fn statfs(&self) -> SysResult<Statfs> {
        self.context.read().statfs()
    }

    fn sync(&self) -> SysResult<()> {
        Ok(())
    }

    fn type_name(&self) -> &'static str {
        "ext4_native"
    }
}

pub struct FileSystem;

impl FileSystemOps for FileSystem {
    fn create(
        &self,
        fsno: u32,
        driver: Option<Arc<dyn BlockDriverOps>>,
        _options: MountOptions,
    ) -> SysResult<Arc<dyn VfsSuperBlockOps>> {
        let driver = driver.ok_or_else(|| mount_errno("FileSystem::create: block driver is None", Errno::EINVAL))?;
        let ctx = Context::from_device(fsno, driver)?;
        Ok(VfsSuperBlock::new(SuperBlock {
            context: Arc::new(SleepRwLockOnStack::new(ctx, "ext4_native::Superblock::context")),
        }))
    }
}
